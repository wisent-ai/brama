use std::process::Command;

use sysinfo::System;
use tracing::{debug, info, warn};

use crate::types::ComputeResources;

/// Bytes in a gibibyte, the unit memory sizes are reported in.
const BYTES_PER_GIB: f64 = 1_073_741_824.0; // https://www.iso.org/standard/87648.html IEC 80000-13 gibi = 2^30
/// Mebibytes in a gibibyte: `nvidia-smi` states memory in MiB.
const MIB_PER_GIB: f64 = 1_024.0; // https://www.iso.org/standard/87648.html IEC 80000-13 gibi / mebi = 2^10

pub fn detect_compute_resources() -> ComputeResources {
    let mut sys = System::new_all();
    sys.refresh_all();

    let ram_gb = sys.total_memory() as f64 / BYTES_PER_GIB;
    let cpu_cores = sys.cpus().len();

    let is_darwin = cfg!(target_os = "macos");
    let is_aarch64 = cfg!(target_arch = "aarch64");
    let is_apple_silicon = is_darwin && is_aarch64;

    let (gpu_type, gpu_name, vram_gb, has_cuda, has_metal) = if is_apple_silicon {
        (
            Some("apple_silicon".into()),
            detect_apple_silicon(),
            None,
            false,
            true,
        )
    } else {
        match detect_nvidia() {
            Some((name, vram)) => (Some("nvidia".into()), Some(name), vram, true, false),
            None => (None, None, None, false, false),
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
        "Detected: GPU={:?} VRAM={:?}GB RAM={:.1}GB cores={}",
        resources.gpu_type, resources.vram_gb, resources.ram_gb, resources.cpu_cores,
    );

    resources
}

/// The chipset `system_profiler` names. Apple Silicon's GPU shares system
/// memory, so no dedicated VRAM is reported for it: RAM is that memory.
fn detect_apple_silicon() -> Option<String> {
    match stado_wait::output(Command::new("system_profiler").arg("SPDisplaysDataType")) {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let name = text
                .lines()
                .find(|l| l.contains("Chipset Model:"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, name)| name.trim().to_string());
            debug!("Apple Silicon detected: {:?}", name);
            name
        }
        Ok(out) => {
            warn!(
                "system_profiler exited {}, so the GPU is not named: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
            None
        }
        Err(e) => {
            warn!("Could not run system_profiler, so the GPU is not named: {e}");
            None
        }
    }
}

/// The first GPU `nvidia-smi` names and its memory in GiB, when it states a
/// number for it; `None` when there is no NVIDIA GPU to ask about.
fn detect_nvidia() -> Option<(String, Option<f64>)> {
    let output = stado_wait::output(Command::new("nvidia-smi").args([
        "--query-gpu=name,memory.total",
        "--format=csv,noheader,nounits",
    ]));
    let out = match output {
        Ok(out) if out.status.success() => out,
        Ok(out) => {
            debug!(
                "nvidia-smi exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return None;
        }
        Err(error) => {
            debug!("nvidia-smi could not be run: {error}");
            return None;
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?;
    let Some((name, memory)) = line.split_once(',') else {
        warn!("nvidia-smi answered a line without a memory column: {line:?}");
        return None;
    };
    let name = name.trim().to_string();
    let vram_mb: f64 = match memory.trim().parse() {
        Ok(vram_mb) => vram_mb,
        Err(error) => {
            warn!("nvidia-smi named {name} with a memory size that is not a number: {error}");
            return Some((name, None));
        }
    };
    let vram_gb = vram_mb / MIB_PER_GIB;
    debug!("NVIDIA GPU: {} with {:.1}GB VRAM", name, vram_gb);
    Some((name, Some(vram_gb)))
}
