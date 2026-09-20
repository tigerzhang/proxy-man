use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use colored::*;
use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, CellAlignment, Color, ContentArrangement, Table};
use futures_util::StreamExt;
use proxy_man_core::config::{
    ConfigStore, HealthCheckConfig, RestartPolicy, RuntimeStatus, ServiceInstance,
    ServiceSettings, ServiceType,
};
use proxy_man_core::drivers::get_driver;
use proxy_man_core::server::{start_server, ServiceWithState};
use proxy_man_core::supervisor::ServiceManager;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const DEFAULT_SERVER_ADDR: &str = "127.0.0.1:8920";

#[derive(Parser)]
#[command(
    name = "proxyman",
    author = "Zhang Hu",
    version,
    about = "ProxyMan: High-performance multi-proxy & DNS service manager & supervisor"
)]
struct Cli {
    #[arg(
        short,
        long,
        default_value = DEFAULT_SERVER_ADDR,
        help = "ProxyMan API daemon address"
    )]
    server: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(alias = "ps", about = "List all proxy services and their status")]
    Status {
        #[arg(short, long, help = "Output raw JSON format")]
        json: bool,
    },

    #[command(about = "Start one or all services")]
    Start {
        #[arg(help = "Service ID, or 'all' to start all configured services")]
        id: String,
    },

    #[command(about = "Stop one or all services")]
    Stop {
        #[arg(help = "Service ID, or 'all' to stop all configured services")]
        id: String,
    },

    #[command(about = "Restart one or all services")]
    Restart {
        #[arg(help = "Service ID, or 'all' to restart all configured services")]
        id: String,
    },

    #[command(about = "View or follow service logs")]
    Logs {
        #[arg(help = "Service ID to view logs for")]
        id: String,
        #[arg(short, long, default_value_t = 100, help = "Number of lines to show")]
        lines: usize,
        #[arg(short = 'f', long = "follow", help = "Follow log stream in real time")]
        follow: bool,
    },

    #[command(about = "Run latency and connectivity probe on a service")]
    Test {
        #[arg(help = "Service ID to probe")]
        id: String,
    },

    #[command(about = "Add a new proxy or DNS service")]
    Add(AddArgs),

    #[command(alias = "rm", about = "Remove a configured service")]
    Remove {
        #[arg(help = "Service ID to delete")]
        id: String,
    },

    #[command(about = "Run the supervisor daemon process")]
    Daemon {
        #[arg(
            short,
            long,
            default_value = DEFAULT_SERVER_ADDR,
            help = "Address to bind HTTP/WebSocket API"
        )]
        bind: String,
        #[arg(long, help = "Path to custom static UI directory to serve")]
        ui_dir: Option<PathBuf>,
    },

    #[command(about = "Run supervisor daemon and launch web dashboard in browser")]
    Web {
        #[arg(
            short,
            long,
            default_value = DEFAULT_SERVER_ADDR,
            help = "Address to bind HTTP/WebSocket API"
        )]
        bind: String,
    },
}

#[derive(Args)]
struct AddArgs {
    #[arg(long, help = "Unique ID for this service")]
    id: String,

    #[arg(long, help = "Display name")]
    name: String,

    #[arg(
        long,
        value_enum,
        help = "Service type: overtls, overtls-chain, clean-dns, gost, custom"
    )]
    service_type: CliServiceType,

    #[arg(long, default_value = "127.0.0.1", help = "Local listen host")]
    listen_host: String,

    #[arg(long, help = "Local listen port")]
    listen_port: u16,

    #[arg(long, help = "Custom binary path if not in standard PATH")]
    bin_path: Option<String>,

    #[arg(
        long,
        help = "Config file path passed to clean-dns as -c (for clean-dns)"
    )]
    config_path: Option<String>,

    #[arg(
        long,
        default_value = "on_failure",
        help = "Restart policy: always, on_failure, never"
    )]
    restart: String,

    // Type-specific arguments
    #[arg(long, help = "Remote server host (for overtls/overtls-chain)")]
    server_host: Option<String>,

    #[arg(
        long,
        default_value_t = 443,
        help = "Remote server port (for overtls)"
    )]
    server_port: u16,

    #[arg(long, help = "Password / pre-shared key (for overtls)")]
    password: Option<String>,

    #[arg(long, help = "Client ID UUID (for overtls)")]
    client_id: Option<String>,

    #[arg(
        long,
        default_value = "/secret-tunnel-path/",
        help = "Tunnel path (for overtls)"
    )]
    tunnel_path: String,

    #[arg(
        long,
        help = "Gost listen spec, e.g. http://:8080 (for gost)"
    )]
    gost_listen: Option<String>,

    #[arg(
        long,
        help = "Gost forward spec, e.g. socks5://127.0.0.1:1080 (for gost)"
    )]
    gost_forward: Option<String>,

    #[arg(long, help = "Custom command executable (for custom)")]
    custom_cmd: Option<String>,

    #[arg(long, num_args = 1.., help = "Custom command arguments (for custom)")]
    custom_args: Vec<String>,
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum CliServiceType {
    Overtls,
    OvertlsChain,
    CleanDns,
    Gost,
    Custom,
}

impl From<CliServiceType> for ServiceType {
    fn from(c: CliServiceType) -> Self {
        match c {
            CliServiceType::Overtls => ServiceType::Overtls,
            CliServiceType::OvertlsChain => ServiceType::OvertlsChain,
            CliServiceType::CleanDns => ServiceType::CleanDns,
            CliServiceType::Gost => ServiceType::Gost,
            CliServiceType::Custom => ServiceType::Custom,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=warn,axum=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer().compact())
        .init();

    let cli = Cli::parse();
    let api_url = format!("http://{}", cli.server);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    match cli.command {
        Commands::Status { json } => handle_status(&client, &api_url, json).await?,
        Commands::Start { id } => handle_start(&client, &api_url, &id).await?,
        Commands::Stop { id } => handle_stop(&client, &api_url, &id).await?,
        Commands::Restart { id } => handle_restart(&client, &api_url, &id).await?,
        Commands::Test { id } => handle_test(&client, &api_url, &id).await?,
        Commands::Logs { id, lines, follow } => {
            handle_logs(&client, &cli.server, &id, lines, follow).await?
        }
        Commands::Add(args) => handle_add(&client, &api_url, args).await?,
        Commands::Remove { id } => handle_remove(&client, &api_url, &id).await?,
        Commands::Daemon { bind, ui_dir } => run_daemon(&bind, ui_dir).await?,
        Commands::Web { bind } => {
            let bind_clone = bind.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(800)).await;
                let url = format!("http://{}", bind_clone);
                let _ = open::that(&url);
            });
            run_daemon(&bind, None).await?;
        }
    }

    Ok(())
}

async fn handle_status(client: &reqwest::Client, api_url: &str, raw_json: bool) -> Result<()> {
    let url = format!("{}/api/services", api_url);
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => {
            // If daemon is not running, fall back to reading local config store directly
            eprintln!(
                "{}",
                "[Daemon not running. Displaying offline configured services]"
                    .yellow()
                    .bold()
            );
            return handle_offline_status(raw_json).await;
        }
    };

    if !resp.status().is_success() {
        anyhow::bail!("API returned error: {}", resp.status());
    }

    let services: Vec<ServiceWithState> = resp.json().await?;

    if raw_json {
        println!("{}", serde_json::to_string_pretty(&services)?);
        return Ok(());
    }

    print_services_table(&services);
    Ok(())
}

async fn handle_offline_status(raw_json: bool) -> Result<()> {
    let store = ConfigStore::new(None)?;
    let instances = store.list().await;

    if raw_json {
        println!("{}", serde_json::to_string_pretty(&instances)?);
        return Ok(());
    }

    let items: Vec<ServiceWithState> = instances
        .into_iter()
        .map(|inst| {
            let id = inst.id.clone();
            ServiceWithState {
                instance: inst,
                runtime: proxy_man_core::config::ServiceRuntimeState {
                    id,
                    status: RuntimeStatus::Stopped,
                    pid: None,
                    uptime_secs: None,
                    restart_count: 0,
                    last_restart_time: None,
                    last_exit_code: None,
                    last_error: None,
                    latency_ms: None,
                    last_probe_time: None,
                    memory_bytes: None,
                },
            }
        })
        .collect();

    print_services_table(&items);
    Ok(())
}

fn print_services_table(services: &[ServiceWithState]) {
    if services.is_empty() {
        println!("{}", "No services configured. Run 'proxyman add --help' to register services.".dimmed());
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("ID").set_alignment(CellAlignment::Left),
            Cell::new("Name").set_alignment(CellAlignment::Left),
            Cell::new("Type").set_alignment(CellAlignment::Center),
            Cell::new("Port").set_alignment(CellAlignment::Right),
            Cell::new("Status").set_alignment(CellAlignment::Center),
            Cell::new("Latency").set_alignment(CellAlignment::Right),
            Cell::new("Uptime").set_alignment(CellAlignment::Right),
            Cell::new("Restarts").set_alignment(CellAlignment::Right),
            Cell::new("PID").set_alignment(CellAlignment::Right),
        ]);

    for s in services {
        let status_cell = match s.runtime.status {
            RuntimeStatus::Running => Cell::new("RUNNING").fg(Color::Green),
            RuntimeStatus::Starting => Cell::new("STARTING").fg(Color::Cyan),
            RuntimeStatus::Degraded => Cell::new("DEGRADED").fg(Color::Yellow),
            RuntimeStatus::Crashed => Cell::new("CRASHED").fg(Color::Red),
            RuntimeStatus::BackoffWaiting => Cell::new("BACKOFF").fg(Color::Magenta),
            RuntimeStatus::Stopped => Cell::new("STOPPED").fg(Color::DarkGrey),
        };

        let latency_cell = if let Some(ms) = s.runtime.latency_ms {
            if ms < 100 {
                Cell::new(format!("{} ms", ms)).fg(Color::Green)
            } else if ms < 300 {
                Cell::new(format!("{} ms", ms)).fg(Color::Yellow)
            } else {
                Cell::new(format!("{} ms", ms)).fg(Color::Red)
            }
        } else {
            Cell::new("-").fg(Color::DarkGrey)
        };

        let uptime_str = if let Some(secs) = s.runtime.uptime_secs {
            format_duration(secs)
        } else {
            "-".to_string()
        };

        let pid_str = s.runtime.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());

        table.add_row(vec![
            Cell::new(&s.instance.id).fg(Color::Cyan),
            Cell::new(&s.instance.name),
            Cell::new(s.instance.service_type.to_string()),
            Cell::new(s.instance.listen_port.to_string()),
            status_cell,
            latency_cell,
            Cell::new(uptime_str),
            Cell::new(s.runtime.restart_count.to_string()),
            Cell::new(pid_str),
        ]);
    }

    println!("{table}");
}

fn format_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

async fn handle_start(client: &reqwest::Client, api_url: &str, id: &str) -> Result<()> {
    if id == "all" {
        let list_url = format!("{}/api/services", api_url);
        let services: Vec<ServiceWithState> = client.get(&list_url).send().await?.json().await?;
        for s in services {
            println!("Starting {}...", s.instance.id.cyan());
            let url = format!("{}/api/services/{}/start", api_url, s.instance.id);
            let _ = client.post(&url).send().await;
        }
        println!("{}", "All services started.".green().bold());
        return Ok(());
    }

    let url = format!("{}/api/services/{}/start", api_url, id);
    let resp = client.post(&url).send().await.context("Failed to connect to daemon")?;
    if resp.status().is_success() {
        println!("{} Service '{}' started successfully.", "✓".green().bold(), id.cyan());
    } else {
        let err_text = resp.text().await?;
        eprintln!("{} Failed to start service: {}", "✗".red().bold(), err_text);
    }
    Ok(())
}

async fn handle_stop(client: &reqwest::Client, api_url: &str, id: &str) -> Result<()> {
    if id == "all" {
        let list_url = format!("{}/api/services", api_url);
        let services: Vec<ServiceWithState> = client.get(&list_url).send().await?.json().await?;
        for s in services {
            println!("Stopping {}...", s.instance.id.cyan());
            let url = format!("{}/api/services/{}/stop", api_url, s.instance.id);
            let _ = client.post(&url).send().await;
        }
        println!("{}", "All services stopped.".green().bold());
        return Ok(());
    }

    let url = format!("{}/api/services/{}/stop", api_url, id);
    let resp = client.post(&url).send().await.context("Failed to connect to daemon")?;
    if resp.status().is_success() {
        println!("{} Service '{}' stopped.", "✓".green().bold(), id.cyan());
    } else {
        let err_text = resp.text().await?;
        eprintln!("{} Failed to stop service: {}", "✗".red().bold(), err_text);
    }
    Ok(())
}

async fn handle_restart(client: &reqwest::Client, api_url: &str, id: &str) -> Result<()> {
    if id == "all" {
        let list_url = format!("{}/api/services", api_url);
        let services: Vec<ServiceWithState> = client.get(&list_url).send().await?.json().await?;
        for s in services {
            println!("Restarting {}...", s.instance.id.cyan());
            let url = format!("{}/api/services/{}/restart", api_url, s.instance.id);
            let _ = client.post(&url).send().await;
        }
        println!("{}", "All services restarted.".green().bold());
        return Ok(());
    }

    let url = format!("{}/api/services/{}/restart", api_url, id);
    let resp = client.post(&url).send().await.context("Failed to connect to daemon")?;
    if resp.status().is_success() {
        println!("{} Service '{}' restarted.", "✓".green().bold(), id.cyan());
    } else {
        let err_text = resp.text().await?;
        eprintln!("{} Failed to restart service: {}", "✗".red().bold(), err_text);
    }
    Ok(())
}

async fn handle_test(client: &reqwest::Client, api_url: &str, id: &str) -> Result<()> {
    println!("Probing service '{}'...", id.cyan());
    let url = format!("{}/api/services/{}/probe", api_url, id);
    let resp = client.post(&url).send().await.context("Failed to connect to daemon")?;
    if resp.status().is_success() {
        let res: serde_json::Value = resp.json().await?;
        let success = res["success"].as_bool().unwrap_or(false);
        let latency = res["latency_ms"].as_u64().unwrap_or(0);
        if success {
            println!(
                "{} Probe SUCCESS: Response time = {} ms",
                "✓".green().bold(),
                latency.to_string().yellow().bold()
            );
        } else {
            let err = res["error"].as_str().unwrap_or("Unknown error");
            eprintln!("{} Probe FAILED: {}", "✗".red().bold(), err);
        }
    } else {
        let err_text = resp.text().await?;
        eprintln!("{} Test request failed: {}", "✗".red().bold(), err_text);
    }
    Ok(())
}

async fn handle_logs(
    client: &reqwest::Client,
    server: &str,
    id: &str,
    lines: usize,
    follow: bool,
) -> Result<()> {
    let api_url = format!("http://{}", server);
    let url = format!("{}/api/services/{}/logs?limit={}", api_url, id, lines);
    let resp = client.get(&url).send().await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let log_lines: Vec<String> = r.json().await?;
            for line in log_lines {
                println!("{}", line);
            }
        }
        _ => {
            // Fallback: read directly from log file
            let store = ConfigStore::new(None)?;
            let log_file = store.get_log_file_path(id);
            if log_file.exists() {
                if let Ok(content) = std::fs::read_to_string(&log_file) {
                    let file_lines: Vec<&str> = content.lines().collect();
                    let start = if file_lines.len() > lines {
                        file_lines.len() - lines
                    } else {
                        0
                    };
                    for line in &file_lines[start..] {
                        println!("{}", line);
                    }
                }
            } else {
                eprintln!("{}", format!("No logs found for service '{}'", id).dimmed());
            }
        }
    }

    if follow {
        println!("{}", "--- Streaming live logs (Press Ctrl+C to stop) ---".dimmed());
        let ws_url = format!("ws://{}/api/services/{}/logs/ws", server, id);
        if let Ok((mut ws_stream, _)) = connect_async(&ws_url).await {
            while let Some(msg) = ws_stream.next().await {
                if let Ok(tokio_tungstenite::tungstenite::Message::Text(text)) = msg {
                    println!("{}", text);
                }
            }
        } else {
            eprintln!("{}", "Unable to connect to live WebSocket log stream. Is daemon running?".yellow());
        }
    }

    Ok(())
}

async fn handle_add(client: &reqwest::Client, api_url: &str, args: AddArgs) -> Result<()> {
    let restart_policy = match args.restart.to_lowercase().as_str() {
        "always" => RestartPolicy::Always,
        "never" => RestartPolicy::Never,
        _ => RestartPolicy::OnFailure,
    };

    let service_type: ServiceType = args.service_type.into();

    let settings = match service_type {
        ServiceType::Overtls => {
            let host = args.server_host.unwrap_or_default();
            let pwd = args.password.unwrap_or_default();
            ServiceSettings::Overtls(proxy_man_core::config::OvertlsSettings {
                remarks: args.name.clone(),
                server_host: host,
                server_port: args.server_port,
                password: pwd,
                client_id: args.client_id,
                tunnel_path: args.tunnel_path,
                ..Default::default()
            })
        }
        ServiceType::OvertlsChain => {
            let host = args.server_host.unwrap_or_default();
            let pwd = args.password.unwrap_or_default();
            ServiceSettings::OvertlsChain(proxy_man_core::config::OvertlsSettings {
                remarks: args.name.clone(),
                server_host: host,
                server_port: args.server_port,
                password: pwd,
                tunnel_path: args.tunnel_path,
                ..Default::default()
            })
        }
        ServiceType::CleanDns => ServiceSettings::CleanDns(proxy_man_core::config::CleanDnsSettings {
            bind: format!("{}:{}", args.listen_host, args.listen_port),
            api_port: 3002,
            config_path: args.config_path,
            ..Default::default()
        }),
        ServiceType::Gost => ServiceSettings::Gost(proxy_man_core::config::GostSettings {
            listen_spec: args.gost_listen.unwrap_or_else(|| {
                format!("http://{}:{}", args.listen_host, args.listen_port)
            }),
            forward_spec: args.gost_forward,
            extra_args: vec![],
            raw_config: None,
        }),
        ServiceType::Custom => ServiceSettings::Custom(proxy_man_core::config::CustomSettings {
            command: args.custom_cmd.unwrap_or_else(|| "echo".to_string()),
            args: args.custom_args,
        }),
    };

    let driver = get_driver(service_type);
    let mut inst = ServiceInstance {
        id: args.id.clone(),
        name: args.name.clone(),
        service_type,
        enabled: true,
        listen_host: args.listen_host,
        listen_port: args.listen_port,
        bin_path: args.bin_path,
        work_dir: None,
        restart_policy,
        max_restart_retries: 5,
        restart_backoff_secs: 2,
        health_check: HealthCheckConfig::default(),
        env_vars: std::collections::HashMap::new(),
        settings,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    inst.health_check = driver.default_health_check(&inst);

    let add_url = format!("{}/api/services", api_url);
    if let Ok(resp) = client.post(&add_url).json(&inst).send().await {
        if resp.status().is_success() {
            println!("{} Service '{}' added via running daemon.", "✓".green().bold(), args.id.cyan());
            return Ok(());
        }
    }

    // Direct store fallback
    let store = ConfigStore::new(None)?;
    store.add(inst).await?;
    println!("{} Service '{}' added to local config store.", "✓".green().bold(), args.id.cyan());
    Ok(())
}

async fn handle_remove(client: &reqwest::Client, api_url: &str, id: &str) -> Result<()> {
    let del_url = format!("{}/api/services/{}", api_url, id);
    if let Ok(resp) = client.delete(&del_url).send().await {
        if resp.status().is_success() {
            println!("{} Service '{}' removed.", "✓".green().bold(), id.cyan());
            return Ok(());
        }
    }

    let store = ConfigStore::new(None)?;
    let res = store.remove(id).await?;
    if res.is_some() {
        println!("{} Service '{}' removed from store.", "✓".green().bold(), id.cyan());
    } else {
        eprintln!("{} Service '{}' not found.", "✗".red().bold(), id);
    }
    Ok(())
}

async fn run_daemon(bind: &str, ui_dir: Option<PathBuf>) -> Result<()> {
    println!(
        "{}",
        r#"
  ____                                 __  __             
 |  _ \ _ __ _____  ___   _   ______ _|  \/  | __ _ _ __  
 | |_) | '__/ _ \ \/ / | | | |______| | |\/| |/ _` | '_ \ 
 |  __/| | | (_) >  <| |_| |        | |  | | (_| | | | |
 |_|   |_|  \___/_/\_\\__, |        |_|  |_|\__,_|_| |_|
                      |___/                             
"#
        .cyan()
        .bold()
    );

    let store = ConfigStore::new(None)?;
    let manager = Arc::new(ServiceManager::new(store.clone()));

    // Auto-start enabled services, adopting leftover processes from a previous run.
    let instances = store.list().await;
    for inst in instances {
        if inst.enabled {
            let mgr = Arc::clone(&manager);
            let id = inst.id.clone();
            tokio::spawn(async move {
                if let Err(e) = mgr.start_service(&id).await {
                    eprintln!("Failed to auto-start service '{}': {}", id, e);
                }
            });
        }
    }

    // Start background watchdog supervisor
    manager.clone().start_watchdog();

    let resolved_ui_dir = if let Some(dir) = ui_dir {
        Some(dir)
    } else {
        let current_ui = std::env::current_dir().map(|d| d.join("ui")).ok();
        if let Some(ref p) = current_ui {
            if p.exists() {
                current_ui
            } else {
                None
            }
        } else {
            None
        }
    };

    let addr: SocketAddr = bind.parse().context("Invalid bind address")?;
    println!("ProxyMan Supervisor Daemon initialized.");
    println!("Listening on: {}", format!("http://{}", addr).green().bold());
    if let Some(ref _d) = resolved_ui_dir {
        println!("Web Console: {}", format!("http://{}/", addr).cyan().underline());
    }

    start_server(manager, addr, resolved_ui_dir).await?;
    Ok(())
}
