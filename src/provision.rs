use std::cell::Cell;
use std::io::{IsTerminal, Write};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::embeddings::OllamaClient;

/// Outcome of a provisioning or status-check run.
pub struct ProvisionResult {
    pub ollama_installed: bool,
    pub ollama_running: bool,
    pub model_available: bool,
    pub errors: Vec<String>,
    pub actions: Vec<String>,
}

/// Check system readiness and bring up what is already installed.
///
/// Never installs software. Will start Ollama and pull models if needed.
pub fn provision(ollama_host: &str, model: &str, on_progress: &dyn Fn(&str)) -> ProvisionResult {
    let mut result = ProvisionResult {
        ollama_installed: false,
        ollama_running: false,
        model_available: false,
        errors: Vec::new(),
        actions: Vec::new(),
    };

    // 1. Check if the ollama binary exists.
    on_progress("Checking for Ollama...");
    result.ollama_installed = check_ollama_binary();

    if !result.ollama_installed {
        result.errors.push(
            "Ollama is not installed. Install it before running init:\n  \
             brew install ollama\n  \
             OR: curl -fsSL https://ollama.com/install.sh | sh\n  \
             OR: snap install ollama"
                .to_string(),
        );
        return result;
    }
    on_progress("  ✓ Ollama found");

    // 2. Check if Ollama is running; start it if not.
    on_progress("Checking if Ollama is running...");
    let client = OllamaClient::new(ollama_host, model);
    result.ollama_running = client.is_healthy();

    if !result.ollama_running {
        on_progress("  Ollama not running, attempting to start...");
        if start_ollama() {
            for _ in 0..15 {
                thread::sleep(Duration::from_secs(1));
                if client.is_healthy() {
                    result.ollama_running = true;
                    result.actions.push("Started Ollama service".to_string());
                    break;
                }
            }
        }

        if !result.ollama_running {
            result.errors.push(
                "Ollama is installed but could not be started. Start it manually:\n  \
                 ollama serve\n  \
                 OR: brew services start ollama\n  \
                 OR: systemctl start ollama"
                    .to_string(),
            );
            return result;
        }
    }
    on_progress("  ✓ Ollama is running");

    // 3. Check if the model is available; pull it if not.
    on_progress(&format!("Checking for model '{model}'..."));
    result.model_available = client.has_model();

    if !result.model_available {
        on_progress(&format!(
            "  Model not found, pulling '{model}' (this may take a minute)..."
        ));

        match pull_with_progress(&client, model) {
            Ok(()) => {
                result.model_available = true;
                result.actions.push(format!("Pulled model '{model}'"));
            }
            Err(e) => {
                result
                    .errors
                    .push(format!("Failed to pull model '{model}': {e}"));
                return result;
            }
        }
    }
    on_progress(&format!("  ✓ Model '{model}' available"));

    result
}

/// Pull a model with TTY-aware progress display.
///
/// On a TTY, shows a single line updated in place with `\r`.
/// On non-TTY (piped/CI), prints a progress line after 1 second, then at most
/// every 10 seconds.
fn pull_with_progress(client: &OllamaClient, model: &str) -> anyhow::Result<()> {
    let is_tty = std::io::stderr().is_terminal();
    let start = Instant::now();
    let last_print = Cell::new(None::<Instant>);

    let result = client.pull_model(&|p| {
        let now = Instant::now();

        if is_tty {
            render_pull_tty(p, model);
        } else {
            render_pull_throttled(p, model, start, now, &last_print);
        }
    });

    if is_tty {
        // Move past the progress line.
        eprintln!();
    }

    result
}

/// Render a single TTY progress line, overwriting in place with `\r`.
fn render_pull_tty(p: &crate::embeddings::PullProgress, model: &str) {
    let line = match (p.completed, p.total) {
        (Some(completed), Some(total)) if total > 0 => {
            let pct = completed.saturating_mul(100) / total;
            format!(
                "\r  Pulling '{model}': {} / {} ({pct}%)",
                format_bytes(completed),
                format_bytes(total),
            )
        }
        _ => {
            let status = p.status.as_deref().unwrap_or("...");
            // Pad to 60 chars to clear remnants of longer previous lines.
            format!("\r  {status:<60}")
        }
    };
    eprint!("{line}");
    let _ = std::io::stderr().flush();
}

/// Render progress for non-TTY output, throttled by time.
///
/// Prints the first line after 1 second, then at most every 10 seconds.
fn render_pull_throttled(
    p: &crate::embeddings::PullProgress,
    model: &str,
    start: Instant,
    now: Instant,
    last_print: &Cell<Option<Instant>>,
) {
    let elapsed = now.duration_since(start);
    let should_print = match last_print.get() {
        None => elapsed >= Duration::from_secs(1),
        Some(prev) => now.duration_since(prev) >= Duration::from_secs(10),
    };

    if !should_print {
        return;
    }

    last_print.set(Some(now));
    match (p.completed, p.total) {
        (Some(completed), Some(total)) if total > 0 => {
            let pct = completed.saturating_mul(100) / total;
            eprintln!(
                "  Pulling '{model}': {} / {} ({pct}%)",
                format_bytes(completed),
                format_bytes(total),
            );
        }
        _ => {
            if let Some(status) = &p.status {
                eprintln!("  {status}");
            }
        }
    }
}

/// Quick read-only health check without side effects.
pub fn check_status(ollama_host: &str, model: &str) -> ProvisionResult {
    let mut result = ProvisionResult {
        ollama_installed: check_ollama_binary(),
        ollama_running: false,
        model_available: false,
        errors: Vec::new(),
        actions: Vec::new(),
    };

    let client = OllamaClient::new(ollama_host, model);
    result.ollama_running = client.is_healthy();
    if result.ollama_running {
        result.model_available = client.has_model();
    }

    result
}

/// Checks whether the `ollama` binary is available by running `ollama --version`.
///
/// Uses `ollama --version` instead of `which ollama` so it works on Windows,
/// macOS, and Linux without relying on a Unix-specific lookup tool.
fn check_ollama_binary() -> bool {
    Command::new("ollama")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Tries to start Ollama using platform-specific service managers, falling back
/// to a direct background spawn.
fn start_ollama() -> bool {
    // Try brew services (macOS).
    if Command::new("brew")
        .args(["services", "start", "ollama"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return true;
    }

    // Try systemctl (Linux).
    if Command::new("systemctl")
        .args(["start", "ollama"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return true;
    }

    // Last resort: spawn as a background process.
    Command::new("ollama")
        .arg("serve")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// Format a byte count as a human-readable string (e.g. "274.0 MB").
fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;

    if b < KB {
        format!("{bytes} B")
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else if b < GB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.1} GB", b / GB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provision_result_defaults() {
        let result = ProvisionResult {
            ollama_installed: false,
            ollama_running: false,
            model_available: false,
            errors: Vec::new(),
            actions: Vec::new(),
        };

        assert!(!result.ollama_installed);
        assert!(!result.ollama_running);
        assert!(!result.model_available);
        assert!(result.errors.is_empty());
        assert!(result.actions.is_empty());
    }

    #[test]
    fn format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn format_bytes_plain_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn format_bytes_kilobytes() {
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
    }

    #[test]
    fn format_bytes_megabytes() {
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(274_000_000), "261.3 MB");
    }

    #[test]
    fn format_bytes_gigabytes() {
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
        assert_eq!(format_bytes(1_288_490_189), "1.2 GB");
    }

    /// Verify that `check_ollama_binary` uses `ollama --version` rather than
    /// `which ollama`.  We inspect the source at compile time — the function
    /// itself is not invoked because we cannot assume Ollama is installed in CI.
    #[test]
    fn check_ollama_binary_uses_version_flag() {
        // Read our own source file and confirm the implementation.
        let src = include_str!("provision.rs");
        assert!(
            src.contains(r#"Command::new("ollama")"#),
            "check_ollama_binary should invoke the ollama binary directly"
        );
        assert!(
            src.contains(r#".arg("--version")"#),
            "check_ollama_binary should pass --version flag"
        );
        // Ensure production code does not use `which` to locate ollama.
        // Build the needle dynamically so this assertion does not match itself.
        let needle = format!("Command::new(\"{}\")", "which");
        assert!(
            !src.contains(&needle),
            "check_ollama_binary must not use `which`"
        );
    }
}
