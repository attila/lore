use std::cell::Cell;
use std::io::{IsTerminal, Write};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::embeddings::{OllamaClient, ProbeError, render_failure};

/// Outcome of the runtime probe that exercises Ollama inference.
///
/// Marked `#[non_exhaustive]` so future variants (e.g. `Cached`) are not
/// breaking changes for downstream `match` consumers.
#[derive(Debug)]
#[non_exhaustive]
pub enum ProbeOutcome {
    /// Probe ran and the model returned a valid embedding.
    Ok,
    /// Probe was not requested — default `lore status` (no `--full`).
    /// Renders as the discoverability hint line.
    NotChecked,
    /// Probe was requested but the prerequisite is missing (no model
    /// manifest on disk). No point asking Ollama to embed without a model.
    Skipped,
    /// Probe ran and Ollama failed to produce an embedding. Carries the
    /// classified error so consumers render a variant-specific message.
    Failed(ProbeError),
}

/// Outcome of a provisioning or status-check run.
pub struct ProvisionResult {
    pub ollama_installed: bool,
    pub ollama_running: bool,
    pub model_available: bool,
    pub runtime: ProbeOutcome,
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
        runtime: ProbeOutcome::NotChecked,
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

    // 4. Verify Ollama can actually run inference. `is_healthy` + `has_model`
    //    only check that the daemon answers and the manifest is on disk —
    //    neither exercises the runner subprocess. The probe loads the model
    //    and runs a single embed against `"."`. `keep_alive: None` lets
    //    Ollama keep the model resident for the default 5 minutes so the
    //    ingest that typically follows `lore init` benefits from the warm
    //    cache.
    on_progress("Verifying inference runtime...");
    match client.probe(None) {
        Ok(()) => {
            on_progress("  ✓ Inference runtime OK");
            result.runtime = ProbeOutcome::Ok;
        }
        Err(err) => {
            let rendered = render_failure(&err);
            result.errors.push(rendered.error_line);
            result.actions.push(rendered.action_line);
            result.runtime = ProbeOutcome::Failed(err);
        }
    }

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

/// Quick health check without filesystem or config side effects.
///
/// When `full` is true, also runs `OllamaClient::probe(Some(0))` to verify
/// the inference runtime — this triggers a model load and immediate unload
/// in Ollama (the `keep_alive: 0` request body), so it has a side effect on
/// Ollama state but not on the filesystem or config.
///
/// Every caller must pass `full` explicitly; there is intentionally no
/// `Default` impl, so a future caller cannot accidentally default to `true`
/// and silently probe on every default `lore status` invocation.
pub fn check_status(ollama_host: &str, model: &str, full: bool) -> ProvisionResult {
    let mut result = ProvisionResult {
        ollama_installed: check_ollama_binary(),
        ollama_running: false,
        model_available: false,
        runtime: ProbeOutcome::NotChecked,
        errors: Vec::new(),
        actions: Vec::new(),
    };

    let client = OllamaClient::new(ollama_host, model);
    result.ollama_running = client.is_healthy();
    if result.ollama_running {
        result.model_available = client.has_model();
    }

    if !full {
        // Default `lore status` — leave `runtime` as `NotChecked` so the
        // renderer shows the discoverability hint pointing at `--full`.
        return result;
    }

    if !result.model_available {
        // No model — no point probing. Renders as omitted Runtime line.
        result.runtime = ProbeOutcome::Skipped;
        return result;
    }

    // `keep_alive: 30` — short enough that one-off `lore status --full`
    // calls don't pin ~270 MB of model in RAM for the OLLAMA_KEEP_ALIVE
    // default (5 minutes), long enough that a "fix Ollama then re-run
    // --full" debug loop hits a warm cache on the second invocation
    // instead of paying the 3–15 s cold load again.
    match client.probe(Some(30)) {
        Ok(()) => result.runtime = ProbeOutcome::Ok,
        Err(err) => result.runtime = ProbeOutcome::Failed(err),
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
        .is_ok_and(|s| s.success())
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
        .is_ok_and(|s| s.success())
    {
        return true;
    }

    // Try systemctl (Linux).
    if Command::new("systemctl")
        .args(["start", "ollama"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
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
    fn provision_result_construction() {
        let result = ProvisionResult {
            ollama_installed: false,
            ollama_running: false,
            model_available: false,
            runtime: ProbeOutcome::NotChecked,
            errors: Vec::new(),
            actions: Vec::new(),
        };

        assert!(!result.ollama_installed);
        assert!(!result.ollama_running);
        assert!(!result.model_available);
        assert!(matches!(result.runtime, ProbeOutcome::NotChecked));
        assert!(result.errors.is_empty());
        assert!(result.actions.is_empty());
    }

    /// `check_status(.., full=false)` is the default `lore status` path. It
    /// must leave `runtime` as `NotChecked` so the renderer shows the
    /// discoverability hint pointing at `--full`. Uses an obviously-unreachable
    /// host so the test does not depend on Ollama being installed.
    #[test]
    fn check_status_not_full_leaves_runtime_not_checked() {
        let result = check_status("http://127.0.0.1:1", "nomic-embed-text", false);
        assert!(matches!(result.runtime, ProbeOutcome::NotChecked));
    }

    /// `check_status(.., full=true)` against unreachable Ollama can't reach
    /// the model manifest, so the probe is skipped — runtime becomes
    /// `Skipped`, not `Failed`. Distinguishes "no model" from "model present
    /// but runner broken" at the type level.
    #[test]
    fn check_status_full_with_unreachable_ollama_skips_probe() {
        let result = check_status("http://127.0.0.1:1", "nomic-embed-text", true);
        assert!(!result.ollama_running);
        assert!(!result.model_available);
        assert!(matches!(result.runtime, ProbeOutcome::Skipped));
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
