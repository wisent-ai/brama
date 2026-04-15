use std::process::Command;

use sysinfo::System;
use tracing::{debug, info, warn};

use crate::types::ComputeResources;

pub fn detect_compute_resources() -> ComputeResources {
    let mut sys = System::new_all();
    sys.refresh_all();

    let ram_gb =
        sys.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0);
    let cpu_cores = sys.cpus().len();

    let is_darwin = cfg!(target_os = "macos");
    let is_aarch64 = cfg!(target_arch = "aarch64");
    let is_apple_silicon = is_darwin && is_aarch64;

    let (gpu_type, gpu_name, vram_gb, has_cuda, has_metal) =
        if is_apple_silicon {
            let (name, vram) = detect_apple_silicon();
            (
                Some("apple_silicon".into()),
                name,
                vram,
                false,
                true,
            )
        } else {
            match detect_nvidia() {
                Some((name, vram)) => (
                    Some("nvidia".into()),
                    Some(name),
                    vram,
                    true,
                    false,
                ),
                None => (None, None, 0.0, false, false),
            }
        };

    let resources = ComputeResources {
        gpu_type,
        gpu_name,
        vram_gb,
        ram_gb,
        cpu_cores,
        has_cuda,
        has_metal,
    };

    info!(
        "Detected: GPU={:?} VRAM={:.1}GB RAM={:.1}GB cores={}",
        resources.gpu_type,
        resources.vram_gb,
        resources.ram_gb,
        resources.cpu_cores,
    );

    resources
}

fn detect_apple_silicon() -> (Option<String>, f64) {
    let output = Command::new("system_profiler")
        .args(["SPDisplaysDataType"])
        .output();

    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let name = text
                .lines()
                .find(|l| l.contains("Chipset Model:"))
                .map(|l| {
                    l.split(':')
                        .nth(1)
                        .unwrap_or("")
                        .trim()
                        .to_string()
                });

            // On Apple Silicon, the unified memory is
            // shared between CPU and GPU. We use total
            // system RAM as a proxy and report 75% of
            // it as available VRAM (typical allocation).
            let mut sys = System::new_all();
            sys.refresh_all();
            let total_gb = sys.total_memory() as f64
                / (1024.0 * 1024.0 * 1024.0);
            let vram = total_gb * 0.75;

            debug!(
                "Apple Silicon detected: {:?}, ~{:.1}GB VRAM",
                name, vram
            );
            (name, vram)
        }
        Err(e) => {
            warn!("Could not run system_profiler: {e}");
            (None, 0.0)
        }
    }
}

fn detect_nvidia() -> Option<(String, f64)> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let line = text.lines().next()?;
            let parts: Vec<&str> =
                line.split(',').map(|s| s.trim()).collect();
            if parts.len() >= 2 {
                let name = parts[0].to_string();
                let vram_mb: f64 =
                    parts[1].parse().unwrap_or(0.0);
                let vram_gb = vram_mb / 1024.0;
                debug!(
                    "NVIDIA GPU: {} with {:.1}GB VRAM",
                    name, vram_gb
                );
                Some((name, vram_gb))
            } else {
                None
            }
        }
        _ => {
            debug!("nvidia-smi not available");
            None
        }
    }
}

/// Given detected compute resources, recommend a model and
/// backend (provider name) for local or cloud inference.
pub fn select_model_for_resources(
    resources: &ComputeResources,
) -> (String, String) {
    if resources.has_metal && resources.vram_gb >= 24.0 {
        return ("qwen3-8b".into(), "local".into());
    }
    if resources.has_metal && resources.vram_gb >= 8.0 {
        return ("qwen3-4b".into(), "local".into());
    }
    if resources.has_cuda && resources.vram_gb >= 24.0 {
        return (
            "deepseek-r1-qwen3-8b".into(),
            "local".into(),
        );
    }
    if resources.has_cuda && resources.vram_gb >= 8.0 {
        return ("qwen3-4b".into(), "local".into());
    }
    // No local GPU capacity: fall back to cloud
    ("gpt-4o-mini".into(), "openai".into())
}
