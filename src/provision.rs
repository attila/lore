use std::process::Command;
use std::thread;
use std::time::Duration;

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
        match client.pull_model(&|p| {
            if let Some(status) = &p.status {
                on_progress(&format!("  {status}"));
            }
        }) {
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
