use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::Manager;
use tauri_plugin_shell::ShellExt;
use tracing;

const SIDECAR_PORT: u16 = 44380;
const HEALTH_POLL_INTERVAL_MS: u64 = 100;
const HEALTH_TIMEOUT_MS: u64 = 10_000;

/// Config returned to the frontend so it can connect to the sidecar
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalServerConfig {
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    #[serde(rename = "authToken")]
    pub auth_token: String,
}

/// Holds the sidecar child process for cleanup on exit
struct SidecarChild(tauri_plugin_shell::process::CommandChild);

struct SidecarState {
    child: Mutex<Option<SidecarChild>>,
    data_dir: std::path::PathBuf,
}

#[tauri::command]
fn get_local_server_config(
    config: tauri::State<'_, LocalServerConfig>,
) -> LocalServerConfig {
    config.inner().clone()
}

#[tauri::command]
fn get_mcp_bridge_path() -> Result<String, String> {
    let exe_path = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_dir = exe_path.parent().ok_or("Cannot determine executable directory")?;
    #[cfg(windows)]
    let bridge_path = exe_dir.join("atomic-mcp-bridge.exe");
    #[cfg(not(windows))]
    let bridge_path = exe_dir.join("atomic-mcp-bridge");
    Ok(bridge_path.to_string_lossy().to_string())
}

const PID_FILE_NAME: &str = "sidecar.pid";

/// Kill a stale sidecar from a previous run using the PID file.
fn kill_stale_sidecar(app_data_dir: &std::path::Path) {
    let pid_file = app_data_dir.join(PID_FILE_NAME);
    if let Ok(contents) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = contents.trim().parse::<u32>() {
            tracing::info!(pid, "Found stale sidecar PID file, killing process");
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill").arg(pid.to_string()).output();
            }
            #[cfg(windows)]
            {
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .output();
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let _ = std::fs::remove_file(&pid_file);
    }
}

/// Write the sidecar PID to disk so we can clean it up on next launch if needed.
fn write_pid_file(app_data_dir: &std::path::Path, pid: u32) {
    let pid_file = app_data_dir.join(PID_FILE_NAME);
    let _ = std::fs::write(&pid_file, pid.to_string());
}

/// Remove the PID file (called on clean shutdown).
fn remove_pid_file(app_data_dir: &std::path::Path) {
    let _ = std::fs::remove_file(app_data_dir.join(PID_FILE_NAME));
}

/// Read or create the local server auth token.
/// Uses the registry (shared across databases) for token management.
fn ensure_local_token(app_data_dir: &std::path::Path) -> String {
    let token_file = app_data_dir.join("local_server_token");

    // Try to read existing token
    if let Ok(token) = std::fs::read_to_string(&token_file) {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return token;
        }
    }

    // Create a new token via the DatabaseManager (opens registry + default db)
    let manager = atomic_core::DatabaseManager::new(app_data_dir)
        .expect("Failed to open database manager for token bootstrap");

    let (_info, raw_token) = manager
        .registry()
        .create_api_token("desktop")
        .expect("Failed to create API token");

    std::fs::write(&token_file, &raw_token)
        .expect("Failed to write local server token file");

    raw_token
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "atomic_lib=info,atomic_core=info,warn".parse().unwrap()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .expect("Failed to get app data directory");

            std::fs::create_dir_all(&app_data_dir)
                .expect("Failed to create app data directory");

            tracing::info!(path = ?app_data_dir, "Data directory");

            // Bootstrap auth token (opens registry/manager)
            let auth_token = ensure_local_token(&app_data_dir);

            let base_url = format!("http://127.0.0.1:{}", SIDECAR_PORT);
            let config = LocalServerConfig {
                base_url: base_url.clone(),
                auth_token: auth_token.clone(),
            };
            app.manage(config.clone());

            // Kill any stale sidecar from a previous run (crash, force quit, etc.)
            kill_stale_sidecar(&app_data_dir);

            // Check if an Atomic server is already running on the port
            let health_url = format!("{}/health", base_url);
            let already_running = reqwest::blocking::Client::new()
                .get(&health_url)
                .timeout(std::time::Duration::from_millis(500))
                .send()
                .is_ok_and(|r| r.status().is_success());

            if already_running {
                tracing::info!(url = %base_url, "Atomic server already running, reusing it");
                app.manage(SidecarState {
                    child: Mutex::new(None),
                    data_dir: app_data_dir.clone(),
                });
            } else {
                // Spawn atomic-server as a sidecar
                let shell = app.shell();
                let sidecar_cmd = shell
                    .sidecar("atomic-server")
                    .expect("Failed to create sidecar command")
                    .args([
                        "--data-dir",
                        app_data_dir.to_str().unwrap(),
                        "serve",
                        "--port",
                        &SIDECAR_PORT.to_string(),
                    ]);

                let (mut rx, child) =
                    sidecar_cmd.spawn().expect("Failed to spawn atomic-server sidecar");

                // Record PID so we can clean up after a crash
                write_pid_file(&app_data_dir, child.pid());

                // Log sidecar (atomic-server) output at info so `npm run tauri dev` shows it without RUST_LOG=debug.
                tauri::async_runtime::spawn(async move {
                    use tauri_plugin_shell::process::CommandEvent;
                    while let Some(event) = rx.recv().await {
                        match event {
                            CommandEvent::Stdout(line) => {
                                tracing::info!(target: "atomic_sidecar", "{}", String::from_utf8_lossy(&line).trim_end());
                            }
                            CommandEvent::Stderr(line) => {
                                tracing::info!(target: "atomic_sidecar", "{}", String::from_utf8_lossy(&line).trim_end());
                            }
                            CommandEvent::Terminated(payload) => {
                                tracing::info!(?payload, "sidecar terminated");
                                break;
                            }
                            CommandEvent::Error(err) => {
                                tracing::debug!(error = %err, "sidecar error");
                            }
                            _ => {}
                        }
                    }
                });

                app.manage(SidecarState {
                    child: Mutex::new(Some(SidecarChild(child))),
                    data_dir: app_data_dir.clone(),
                });

                // Poll health endpoint until ready
                let start = std::time::Instant::now();
                loop {
                    if start.elapsed().as_millis() as u64 > HEALTH_TIMEOUT_MS {
                        tracing::warn!(timeout_ms = HEALTH_TIMEOUT_MS, "Sidecar health check timed out");
                        break;
                    }
                    match reqwest::blocking::Client::new()
                        .get(&health_url)
                        .timeout(std::time::Duration::from_millis(500))
                        .send()
                    {
                        Ok(resp) if resp.status().is_success() => {
                            tracing::debug!(url = %base_url, elapsed_ms = start.elapsed().as_millis(), "Sidecar ready");
                            break;
                        }
                        _ => {
                            std::thread::sleep(std::time::Duration::from_millis(HEALTH_POLL_INTERVAL_MS));
                        }
                    }
                }
            }

            #[cfg(debug_assertions)]
            if let Some(win) = app.get_webview_window("main") {
                win.open_devtools();
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_local_server_config,
            get_mcp_bridge_path,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Kill sidecar on app exit and clean up PID file
                if let Some(state) = app.try_state::<SidecarState>() {
                    if let Ok(mut child_opt) = state.child.lock() {
                        if let Some(SidecarChild(child)) = child_opt.take() {
                            tracing::info!("Shutting down sidecar");
                            let _ = child.kill();
                        }
                    }
                    remove_pid_file(&state.data_dir);
                }
            }
        });
}
