//! Just enough protobuf and gzip to read OTLP/HTTP protobuf batches, which is what most SDKs send
//! by default. The decoder maps the fields antsy uses onto the OTLP/JSON shape, so the rest of
//! antsy only ever sees JSON.

use serde_json::{json, Map, Value};

enum Field<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32,
}

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*i)?;
        *i += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn fields(b: &[u8]) -> Vec<(u64, Field<'_>)> {
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let Some(key) = varint(b, &mut i) else { break };
        let f = match key & 7 {
            0 => match varint(b, &mut i) {
                Some(v) => Field::Varint(v),
                None => break,
            },
            1 if i + 8 <= b.len() => {
                i += 8;
                Field::Fixed64(u64::from_le_bytes(b[i - 8..i].try_into().unwrap()))
            }
            2 => {
                let Some(n) = varint(b, &mut i) else { break };
                let end = i.saturating_add(n as usize);
                if end > b.len() {
                    break;
                }
                i = end;
                Field::Bytes(&b[end - n as usize..end])
            }
            5 if i + 4 <= b.len() => {
                i += 4;
                Field::Fixed32
            }
            _ => break,
        };
        out.push((key >> 3, f));
    }
    out
}

fn text(b: &[u8]) -> Value {
    Value::String(String::from_utf8_lossy(b).into_owned())
}

fn hexs(b: &[u8]) -> Value {
    Value::String(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn any_value(b: &[u8]) -> Value {
    for (n, f) in fields(b) {
        match (n, f) {
            (1, Field::Bytes(s)) => return json!({"stringValue": text(s)}),
            (2, Field::Varint(v)) => return json!({"boolValue": v != 0}),
            (3, Field::Varint(v)) => return json!({"intValue": (v as i64).to_string()}),
            (4, Field::Fixed64(v)) => return json!({"doubleValue": f64::from_bits(v)}),
            _ => {}
        }
    }
    json!({})
}

fn key_value(b: &[u8]) -> Value {
    let mut kv = json!({"key": "", "value": {}});
    for (n, f) in fields(b) {
        match (n, f) {
            (1, Field::Bytes(s)) => kv["key"] = text(s),
            (2, Field::Bytes(s)) => kv["value"] = any_value(s),
            _ => {}
        }
    }
    kv
}

fn attributes_of(b: &[u8], field: u64) -> Value {
    Value::Array(
        fields(b)
            .into_iter()
            .filter_map(|(n, f)| match f {
                Field::Bytes(s) if n == field => Some(key_value(s)),
                _ => None,
            })
            .collect(),
    )
}

fn resource(b: &[u8]) -> Value {
    json!({"attributes": attributes_of(b, 1)})
}

fn scope(b: &[u8]) -> Value {
    let mut m = Map::new();
    for (n, f) in fields(b) {
        if let (1, Field::Bytes(s)) = (n, f) {
            m.insert("name".into(), text(s));
        }
    }
    Value::Object(m)
}

fn span(b: &[u8]) -> Value {
    let mut sp = json!({"attributes": attributes_of(b, 9)});
    for (n, f) in fields(b) {
        match (n, f) {
            (1, Field::Bytes(s)) => sp["traceId"] = hexs(s),
            (2, Field::Bytes(s)) => sp["spanId"] = hexs(s),
            (4, Field::Bytes(s)) => sp["parentSpanId"] = hexs(s),
            (5, Field::Bytes(s)) => sp["name"] = text(s),
            (6, Field::Varint(v)) => sp["kind"] = json!(v),
            (7, Field::Fixed64(v)) => sp["startTimeUnixNano"] = json!(v.to_string()),
            (8, Field::Fixed64(v)) => sp["endTimeUnixNano"] = json!(v.to_string()),
            (15, Field::Bytes(s)) => {
                let mut st = json!({});
                for (n, f) in fields(s) {
                    match (n, f) {
                        (2, Field::Bytes(m)) => st["message"] = text(m),
                        (3, Field::Varint(c)) => st["code"] = json!(c),
                        _ => {}
                    }
                }
                sp["status"] = st;
            }
            _ => {}
        }
    }
    sp
}

fn log_record(b: &[u8]) -> Value {
    let mut r = json!({"attributes": attributes_of(b, 6)});
    for (n, f) in fields(b) {
        match (n, f) {
            (1, Field::Fixed64(v)) => r["timeUnixNano"] = json!(v.to_string()),
            (2, Field::Varint(v)) => r["severityNumber"] = json!(v),
            (3, Field::Bytes(s)) => r["severityText"] = text(s),
            (5, Field::Bytes(s)) => r["body"] = any_value(s),
            _ => {}
        }
    }
    r
}

/// resource (1) + repeated scope groups (2), each scope (1) + repeated items (2).
fn resource_groups(b: &[u8], group_key: &str, items_key: &str, item: fn(&[u8]) -> Value) -> Value {
    let mut out = vec![];
    for (n, f) in fields(b) {
        let (1, Field::Bytes(rs)) = (n, f) else { continue };
        let mut r = json!({"resource": {"attributes": []}});
        let mut groups = vec![];
        for (n, f) in fields(rs) {
            match (n, f) {
                (1, Field::Bytes(s)) => r["resource"] = resource(s),
                (2, Field::Bytes(ss)) => {
                    let mut g = json!({});
                    let mut items = vec![];
                    for (n, f) in fields(ss) {
                        match (n, f) {
                            (1, Field::Bytes(s)) => g["scope"] = scope(s),
                            (2, Field::Bytes(s)) => items.push(item(s)),
                            _ => {}
                        }
                    }
                    g[items_key] = Value::Array(items);
                    groups.push(g);
                }
                _ => {}
            }
        }
        r[group_key] = Value::Array(groups);
        out.push(r);
    }
    Value::Array(out)
}

/// An OTLP protobuf export request as OTLP/JSON. `kind` is "traces" or "logs"; metrics aren't decoded.
pub fn decode(kind: &str, body: &[u8]) -> Option<Value> {
    match kind {
        "traces" => Some(json!({"resourceSpans": resource_groups(body, "scopeSpans", "spans", span)})),
        "logs" => Some(json!({"resourceLogs": resource_groups(body, "scopeLogs", "logRecords", log_record)})),
        _ => None,
    }
}

/// Undo `Content-Encoding: gzip` (RFC 1952 header, then raw deflate).
pub fn gunzip(b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < 18 || b[0] != 0x1f || b[1] != 0x8b || b[2] != 8 {
        return None;
    }
    let flags = b[3];
    let mut i = 10;
    if flags & 4 != 0 {
        let n = u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize;
        i += 2 + n;
    }
    for bit in [8, 16] {
        if flags & bit != 0 {
            i += b.get(i..)?.iter().position(|&c| c == 0)? + 1;
        }
    }
    if flags & 2 != 0 {
        i += 2;
    }
    miniz_oxide::inflate::decompress_to_vec_with_limit(b.get(i..)?, 256 << 20).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(n: u64, wt: u64) -> Vec<u8> {
        vec![((n << 3) | wt) as u8]
    }
    fn bytes(n: u64, b: &[u8]) -> Vec<u8> {
        let mut v = tag(n, 2);
        v.push(b.len() as u8);
        v.extend_from_slice(b);
        v
    }

    #[test]
    fn decodes_an_error_span() {
        let kv = [bytes(1, b"service.name"), bytes(2, &bytes(1, b"api"))].concat();
        let res = bytes(1, &kv);
        let status = [bytes(2, b"boom"), tag(3, 0), vec![2]].concat();
        let span = [bytes(5, b"GET /x"), bytes(15, &status)].concat();
        let scope_spans = bytes(2, &span);
        let rs = [bytes(1, &res), bytes(2, &scope_spans)].concat();
        let req = bytes(1, &rs);
        let v = decode("traces", &req).unwrap();
        let sp = &v["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(sp["name"], "GET /x");
        assert_eq!(sp["status"]["code"], 2);
        assert_eq!(sp["status"]["message"], "boom");
        assert_eq!(v["resourceSpans"][0]["resource"]["attributes"][0]["value"]["stringValue"], "api");
    }
}
