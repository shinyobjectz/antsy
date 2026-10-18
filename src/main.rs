mod board;
mod config;
mod daemon;
mod feed;
mod hook;
mod mem;
mod otel;
mod run;
mod spend;
mod util;
mod watch;

const HELP: &str = "antsy — fail fast, never wait on a clock.

  antsy run [--name N] [--stall 2m] [--max 2h] [--mem 8G] [--need-mem 4G] [--fail-on RE]...
            [--no-default-fail-on] [--grace 3s] [--progress-file F] [--cost-usd 0.40] [-q] -- <cmd>
      Run under a failure-mode contract. Exit codes: child's own, 121 stall, 122 mem, 123 fail_on,
      124 max, 125 refused at admission, 126 stopped, 128+N signal.
  antsy watch pid|log|check …   failure modes for things antsy didn't start (evaluated by the daemon)
  antsy wait [ID…]               block until those (or this session's work) end; exit 1 on a failure
  antsy events [-f] [--all] [-n N]   failures and results; -f streams them (for the Monitor tool)
  antsy board [--mine]           memory, every agent's running jobs and watches, today's spend
  antsy log ID [-n N]            a job's outcome and log tail
  antsy stop ID… | --mine        stop jobs, remove watches
  antsy spend [USD WHAT]         log money spent testing; exits 3 over the daily cap
  antsy mem                      one line of machine memory
  antsy creed                    the operating rules the plugin gives every session
  antsy doctor                   check the install
  antsy daemon                   run the watcher in the foreground (normally started for you)
  antsy hook <event>             Claude Code hook entry point

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
