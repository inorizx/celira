//! 外部输入入口的独立命令：`add` / `list` / `remove` / `test`。
//!
//! 它是**操作者的工具**，不是主体的动词：它改的是 `<环境根>/channels/routes.json`（哪几个外部
//! 系统能说话、各带哪把凭据），也可以对着**正在跑的入口**发一条真请求。
//!
//! 边界：
//!
//! - **它不启动入口**（入口由宿主决定起不起；本命令只改配置、或对已经起来的那个发一条）；
//! - **secret 只打印一次**：`add` 把它打到 stdout，此后 `list` 只说"这一条在、开没开"，
//!   不重复说凭据；
//! - **`remove` 先校验 route id 形状**：`../` 一类的值在碰文件之前就被拒，写不出环境根；
//! - 参数是手写解析（与 `agent_host` / `agent_host_status` 同款），不引 `clap`。
//!
//! 退出码（与其它宿主命令同一条纪律：退出码是读数）：
//!
//! - `0` 做完了；
//! - `1` 用法错（参数形状不对 / 未知子命令）——**一个字节都没写**；
//! - `2` 配置 / IO 出错（读不出来、写不下去、没有这条路由）；
//! - `3` 请求没发出去 / 没收到响应（`test` 那一条；收到了就照状态码报，无论 2xx 与否）。

use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use celira_agent::channels::{self, ChannelConfig};

const USAGE: &str = "\
用法:
  agent_channel add    --root <环境根> --label <标签> [--max-body N] [--rate N] [--bind 地址]
  agent_channel list   --root <环境根>
  agent_channel remove --root <环境根> --route <route>
  agent_channel test   --root <环境根> --route <route> [--body TEXT]
  agent_channel --help

说明:
  add     生成 route id（label 的 slug，冲突加 -2 / -3）与 secret（cc_ + 32 字节随机数的
          十六进制），写 <环境根>/channels/routes.json（Unix 0600）。secret 只打印这一次。
          缺省 max-body 32768（硬上限 262144）、rate 60（0 = 不限）、bind 127.0.0.1:8787。
          label 不是路径：含 / \\ 或 .. 的 label 会被拒绝。
  list    列出路由（id / label / 开关 / 上限 / 限流），不打印 secret。
  remove  删一条路由；形状不对的 route id 直接拒绝，不碰任何文件。
  test    对着配置里那个 bind 地址发一条 Bearer + Idempotency-Key 请求，打印状态码与响应体。
          请求没发出去是退出码 3；收到了响应就是 0（状态码本身是读数）。

  它只改配置、或用配置发请求：不启动入口、不落任何事件文件（test 那一条由入口落）。
";

/// 一条子命令。
enum Command {
    Add {
        root: String,
        label: String,
        max_body: Option<usize>,
        rate: Option<u32>,
        bind: Option<String>,
    },
    List {
        root: String,
    },
    Remove {
        root: String,
        route: String,
    },
    Test {
        root: String,
        route: String,
        body: String,
    },
}

fn parse(args: &[String]) -> Result<Option<Command>, String> {
    let Some(name) = args.first() else {
        return Err("缺子命令".to_string());
    };
    if name == "--help" || name == "-h" {
        return Ok(None);
    }
    let mut root: Option<String> = None;
    let mut label: Option<String> = None;
    let mut route: Option<String> = None;
    let mut max_body: Option<usize> = None;
    let mut rate: Option<u32> = None;
    let mut bind: Option<String> = None;
    let mut body: Option<String> = None;

    let mut index = 1;
    while index < args.len() {
        let argument = args[index].as_str();
        match argument {
            "--help" | "-h" => return Ok(None),
            "--root" => root = Some(next_value(args, &mut index, "--root")?),
            "--label" => label = Some(next_value(args, &mut index, "--label")?),
            "--route" => route = Some(next_value(args, &mut index, "--route")?),
            "--bind" => bind = Some(next_value(args, &mut index, "--bind")?),
            "--body" => body = Some(next_value(args, &mut index, "--body")?),
            "--max-body" => {
                let text = next_value(args, &mut index, "--max-body")?;
                max_body = Some(
                    text.parse()
                        .map_err(|_| format!("--max-body 不是整数: {text}"))?,
                );
            }
            "--rate" => {
                let text = next_value(args, &mut index, "--rate")?;
                rate = Some(
                    text.parse()
                        .map_err(|_| format!("--rate 不是整数: {text}"))?,
                );
            }
            other if other.starts_with("--") => {
                return Err(format!("未知参数: {other}"));
            }
            other => return Err(format!("多余的位置参数: {other}")),
        }
        index += 1;
    }

    let root = root.ok_or_else(|| "缺 --root".to_string())?;
    match name.as_str() {
        "add" => Ok(Some(Command::Add {
            root,
            label: label.ok_or_else(|| "add 缺 --label".to_string())?,
            max_body,
            rate,
            bind,
        })),
        "list" => {
            if label.is_some() || route.is_some() || max_body.is_some() || rate.is_some() {
                return Err("list 只认 --root".to_string());
            }
            Ok(Some(Command::List { root }))
        }
        "remove" => {
            if label.is_some() || max_body.is_some() || rate.is_some() || bind.is_some() {
                return Err("remove 只认 --root 与 --route".to_string());
            }
            Ok(Some(Command::Remove {
                root,
                route: route.ok_or_else(|| "remove 缺 --route".to_string())?,
            }))
        }
        "test" => {
            if label.is_some() || max_body.is_some() || rate.is_some() || bind.is_some() {
                return Err("test 只认 --root、--route 与 --body".to_string());
            }
            Ok(Some(Command::Test {
                root,
                route: route.ok_or_else(|| "test 缺 --route".to_string())?,
                body: body.unwrap_or_else(|| "ping".to_string()),
            }))
        }
        other => Err(format!("未知子命令: {other}")),
    }
}

fn next_value(args: &[String], index: &mut usize, name: &str) -> Result<String, String> {
    *index += 1;
    let value = args
        .get(*index)
        .ok_or_else(|| format!("{name} 后面要给一个值"))?
        .clone();
    if value.is_empty() {
        return Err(format!("{name} 的值不能为空"));
    }
    Ok(value)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse(&args) {
        Ok(Some(command)) => command,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(detail) => {
            eprintln!("{detail}");
            eprint!("{USAGE}");
            return ExitCode::from(1);
        }
    };

    match command {
        Command::Add {
            root,
            label,
            max_body,
            rate,
            bind,
        } => match channels::add_route(Path::new(&root), &label, max_body, rate, bind.as_deref()) {
            Ok(added) => {
                println!("route={}", added.id);
                println!("endpoint={}", added.endpoint);
                println!("secret={}", added.secret);
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("加不了路由: {error}");
                ExitCode::from(2)
            }
        },
        Command::List { root } => match ChannelConfig::load(Path::new(&root)) {
            Ok(Some(config)) => {
                println!("routes: {}  bind={}", config.routes().len(), config.bind());
                for route in config.routes() {
                    println!(
                        "- id={} label={} enabled={} max_body={} rate={}",
                        route.id(),
                        route.label(),
                        route.enabled(),
                        route.max_body_bytes(),
                        route.rate_per_minute(),
                    );
                }
                ExitCode::SUCCESS
            }
            Ok(None) => {
                println!("routes: 0（没有 routes.json：入口默认关闭）");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("读不了配置: {error}");
                ExitCode::from(2)
            }
        },
        Command::Remove { root, route } => match channels::remove_route(Path::new(&root), &route) {
            Ok(true) => {
                println!("removed=true route={route}");
                ExitCode::SUCCESS
            }
            Ok(false) => {
                eprintln!("没有这条路由: {route}（未改动配置）");
                ExitCode::from(2)
            }
            Err(error) => {
                eprintln!("删不了路由: {error}");
                ExitCode::from(2)
            }
        },
        Command::Test { root, route, body } => test_route(Path::new(&root), &route, &body),
    }
}

/// 对着配置里的 bind 地址发一条真请求。
///
/// `0.0.0.0` / `::` 这种"所有本机地址"的声明，连接时换成回环——入口听在所有地址上，
/// 而命令是从本机发的。端口 `0` 连不上（那是测试用的声明），如实报成配置错误。
fn test_route(root: &Path, route_id: &str, body: &str) -> ExitCode {
    let config = match ChannelConfig::load(root) {
        Ok(Some(config)) => config,
        Ok(None) => {
            eprintln!("没有 routes.json（入口默认关闭）");
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("读不了配置: {error}");
            return ExitCode::from(2);
        }
    };
    let Some(route) = config.routes().iter().find(|route| route.id() == route_id) else {
        eprintln!("没有这条路由: {route_id}");
        return ExitCode::from(2);
    };
    let Ok(address) = config.bind().parse::<SocketAddr>() else {
        eprintln!("bind 不是 ip:port: {}", config.bind());
        return ExitCode::from(2);
    };
    if address.port() == 0 {
        eprintln!("bind 的端口是 0（测试用声明），没有可以连接的地址");
        return ExitCode::from(2);
    }
    let host = if address.ip().is_unspecified() {
        match address {
            SocketAddr::V4(_) => "127.0.0.1".to_string(),
            SocketAddr::V6(_) => "[::1]".to_string(),
        }
    } else {
        address.ip().to_string()
    };
    let url = format!("http://{host}:{}/channels/{route_id}", address.port());

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("建不了请求客户端: {error}");
            return ExitCode::from(3);
        }
    };
    let key = format!(
        "cli-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", route.secret()))
        .header("Idempotency-Key", key)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(body.to_string())
        .send();
    match response {
        Ok(response) => {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            println!("status={}", status.as_u16());
            println!("body={text}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("请求没发出去: {error}");
            ExitCode::from(3)
        }
    }
}
