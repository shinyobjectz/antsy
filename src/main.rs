mod account;
mod board;
mod config;
mod daemon;
mod feed;
mod hook;
mod init;
mod mem;
mod otel;
mod pb;
mod run;
mod spend;
mod traces;
mod util;
mod watch;

const HELP: &str = "antsy — fail fast, never wait on a clock.

  antsy run [--name N] [--stall 2m] [--max 2h] [--mem 8G] [--need-mem 4G] [--fail-on RE]...
            [--no-default-fail-on] [--grace 3s] [--mem auto] [--progress-file F] [--cost-usd 0.40] [--for PURPOSE] [-q] -- <cmd>
      Run under a failure-mode contract. Exit codes: child's own, 121 stall, 122 mem, 123 fail_on,
      124 max, 125 refused at admission, 126 stopped, 128+N signal.
  antsy watch pid|log|check …   failure modes for things antsy didn't start (evaluated by the daemon)
  antsy wait [ID…]               block until those (or this session's work) end; exit 1 on a failure
  antsy events [-f] [--all] [-n N]   failures and results; -f streams them (for the Monitor tool)
  antsy board [--mine]           memory, every agent's running jobs and watches, today's spend
  antsy log ID [-n N]            a job's outcome and log tail
  antsy stop ID… | --mine        stop jobs, remove watches
  antsy spend [USD WHAT] [--for P]   log money spent testing; exits 3 over the daily cap
  antsy note ID|PURPOSE TEXT [--useful|--waste]   record what a run or purpose bought
  antsy account [--days 7] [--by for|name|mode|session] [--json]
                                 where compute and money went, failure loops, purposes with no outcome
  antsy traces [--job ID] [--errors] [--service S] [--slow MS] [--days 1] [-n 30] [--json]
                                 spans the daemon received and jobs recorded
  antsy history [NAME] [-n 15]   past runs: outcome, duration, peak memory (what `--mem auto` learns from)
  antsy config                   effective settings and their sources
  antsy mem                      one line of machine memory
  antsy init [--write]           scan this project (kinds, running processes, OTel, paid SDKs, CI) and draft .antsy.json
  antsy creed                    the operating rules the plugin gives every session
  antsy doctor                   check the install
  antsy daemon                   run the watcher in the foreground (normally started for you)
  antsy hook <event>             Claude Code hook entry point

ANTSY_FOR in the environment sets the default purpose for runs and spend.
State lives in ~/.antsy (ANTSY_HOME). Config: ~/.antsy/config.json, then the nearest .antsy.json.";

fn doctor() -> i32 {
    let c = config::load();
    println!("antsy {}", env!("CARGO_PKG_VERSION"));
    println!("home {}", util::home().display());
    println!("{}", mem::sys().line());
    match mem::external_killer(&config::strs(&c, "external_killers")) {
        Some(k) => println!("external memory killer: {k} (antsy will not kill at the memory edge)"),
        None => println!("no external memory killer; mem_killer={}", config::get_str(&c, "mem_killer")),
    }
    daemon::ensure();
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!("daemon {}", if daemon::running() { "up" } else { "DOWN (see ~/.antsy/daemon.log)" });
    let port = config::get_str(&c, "otlp_port");
    let ok = std::net::TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().unwrap(), std::time::Duration::from_millis(300)).is_ok();
    println!("OTLP/JSON receiver on 127.0.0.1:{port}: {}", if ok { "listening" } else { "not reachable" });
    println!("session {}", util::session());
    0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = if args.is_empty() { &[][..] } else { &args[1..] };
    let code = match args.first().map(String::as_str).unwrap_or("help") {
        "run" => run::main(rest),
        "watch" => watch::main(rest),
        "wait" => feed::wait(rest),
        "events" => feed::events(rest),
        "board" | "status" => board::main(rest),
        "log" => feed::log(rest),
        "stop" => feed::stop(rest),
        "spend" => spend::main(rest),
        "mem" => {
            println!("{}", mem::sys().line());
            0
        }
        "creed" => {
            println!("{}", hook::CREED);
            0
        }
        "doctor" => doctor(),
        "init" => init::main(rest),
        "note" => account::note(rest),
        "account" => account::main(rest),
        "history" => account::history(rest),
        "traces" => traces::main(rest),
        "config" => config::main(rest),
        "daemon" => daemon::main(rest),
        "hook" => hook::main(rest),
        "version" | "--version" | "-V" => {
            println!("antsy {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "help" | "--help" | "-h" => {
            println!("{HELP}");
            0
        }
        other => {
            eprintln!("antsy: unknown command {other}\n\n{HELP}");
            2
        }
    };
    std::process::exit(code);
}
