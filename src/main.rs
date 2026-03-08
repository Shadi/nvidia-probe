use std::ffi::{CStr, c_char, c_uint, c_ulonglong, c_void};
use std::fs;
use std::path::Path;

use libloading::{Library, Symbol};

const NVML_SUCCESS: c_uint = 0;
const NVML_DEVICE_NAME_BUFFER_SIZE: usize = 64;

#[repr(C)]
#[derive(Default)]
struct NvmlMemory {
    total: c_ulonglong,
    free: c_ulonglong,
    used: c_ulonglong,
}

#[repr(C)]
#[derive(Default)]
struct NvmlUtilization {
    gpu: c_uint,
    memory: c_uint,
}

type NvmlDevice = *mut c_void;

/// Call an NVML function, returning `Some(result)` on success or `None` on failure.
unsafe fn nvml_call<T: Default>(f: impl FnOnce(*mut T) -> c_uint) -> Option<T> {
    let mut val = T::default();
    if f(&mut val) == NVML_SUCCESS {
        Some(val)
    } else {
        None
    }
}

/// Read a sysfs attribute file, returning the trimmed contents.
fn sysfs_read(path: &Path, attr: &str) -> Option<String> {
    let s = fs::read_to_string(path.join(attr)).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

const NVML_SEARCH_PATHS: &[&str] = &[
    "libnvidia-ml.so.1",
    "libnvidia-ml.so",
    // Host-mounted paths (for container usage with -v /:/host:ro)
    "/host/usr/lib/libnvidia-ml.so.1",
    "/host/usr/lib64/libnvidia-ml.so.1",
    "/host/usr/lib/x86_64-linux-gnu/libnvidia-ml.so.1",
    "/host/usr/lib/aarch64-linux-gnu/libnvidia-ml.so.1",
    // GKE / cos nodes
    "/host/home/kubernetes/bin/nvidia/lib64/libnvidia-ml.so.1",
];

struct Nvml {
    _lib: Library,
}

struct GpuInfo {
    index: c_uint,
    name: String,
    memory: Option<(c_ulonglong, c_ulonglong, c_ulonglong)>, // total, used, free
    temperature: Option<c_uint>,
    utilization: Option<(c_uint, c_uint)>, // gpu%, mem%
}

struct PciGpu {
    slot: String,
    device_name: String,
}

impl Nvml {
    fn load() -> Option<Self> {
        let lib = NVML_SEARCH_PATHS
            .iter()
            .find_map(|path| unsafe { Library::new(*path) }.ok())?;

        let init: Symbol<unsafe extern "C" fn() -> c_uint> =
            unsafe { lib.get(b"nvmlInit_v2") }.ok()?;
        if unsafe { init() } != NVML_SUCCESS {
            return None;
        }

        Some(Nvml { _lib: lib })
    }

    fn sym<T>(&self, name: &[u8]) -> Symbol<'_, T> {
        unsafe { self._lib.get(name) }.expect("missing NVML symbol")
    }

    fn query_gpus(&self) -> Vec<GpuInfo> {
        let get_count = self.sym::<unsafe extern "C" fn(*mut c_uint) -> c_uint>(b"nvmlDeviceGetCount_v2");
        let get_handle = self.sym::<unsafe extern "C" fn(c_uint, *mut NvmlDevice) -> c_uint>(b"nvmlDeviceGetHandleByIndex_v2");
        let get_name = self.sym::<unsafe extern "C" fn(NvmlDevice, *mut c_char, c_uint) -> c_uint>(b"nvmlDeviceGetName");
        let get_memory = self.sym::<unsafe extern "C" fn(NvmlDevice, *mut NvmlMemory) -> c_uint>(b"nvmlDeviceGetMemoryInfo");
        let get_temp = self.sym::<unsafe extern "C" fn(NvmlDevice, c_uint, *mut c_uint) -> c_uint>(b"nvmlDeviceGetTemperature");
        let get_util = self.sym::<unsafe extern "C" fn(NvmlDevice, *mut NvmlUtilization) -> c_uint>(b"nvmlDeviceGetUtilizationRates");

        let count = match unsafe { nvml_call(|p| get_count(p)) } {
            Some(c) => c,
            None => return Vec::new(),
        };

        (0..count)
            .filter_map(|i| {
                let device = unsafe { nvml_call(|p| get_handle(i, p)) }?;

                let mut name_buf = [0u8; NVML_DEVICE_NAME_BUFFER_SIZE];
                let name = if unsafe {
                    get_name(
                        device,
                        name_buf.as_mut_ptr() as *mut c_char,
                        NVML_DEVICE_NAME_BUFFER_SIZE as c_uint,
                    )
                } == NVML_SUCCESS
                {
                    unsafe { CStr::from_ptr(name_buf.as_ptr() as *const c_char) }
                        .to_string_lossy()
                        .into_owned()
                } else {
                    "Unknown".to_string()
                };

                let memory = unsafe {
                    nvml_call::<NvmlMemory>(|p| get_memory(device, p))
                }
                .map(|m| (m.total, m.used, m.free));

                let temperature = unsafe {
                    nvml_call(|p| get_temp(device, 0, p))
                };

                let utilization = unsafe {
                    nvml_call::<NvmlUtilization>(|p| get_util(device, p))
                }
                .map(|u| (u.gpu, u.memory));

                Some(GpuInfo { index: i, name, memory, temperature, utilization })
            })
            .collect()
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        if let Ok(shutdown) = unsafe {
            self._lib
                .get::<unsafe extern "C" fn() -> c_uint>(b"nvmlShutdown")
        } {
            unsafe { shutdown() };
        }
    }
}

fn format_bytes(bytes: c_ulonglong) -> String {
    let mib = bytes / (1024 * 1024);
    if mib >= 1024 {
        format!("{:.1} GiB", mib as f64 / 1024.0)
    } else {
        format!("{mib} MiB")
    }
}

fn scan_pci_gpus() -> Vec<PciGpu> {
    // Try native path first, then host-mounted path (container with -v /:/host:ro)
    let pci_dir = ["/sys/bus/pci/devices", "/host/sys/bus/pci/devices"]
        .iter()
        .find(|p| Path::new(p).exists());
    let pci_dir = match pci_dir {
        Some(p) => Path::new(p),
        None => return Vec::new(),
    };
    let entries = match fs::read_dir(pci_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if sysfs_read(&path, "vendor")?.as_str() != "0x10de" {
                return None;
            }
            // class 0x03xxxx = display controller
            let class = sysfs_read(&path, "class")?;
            if !class.trim_start_matches("0x").starts_with("03") {
                return None;
            }

            let slot = entry.file_name().to_string_lossy().into_owned();
            let device_id = sysfs_read(&path, "device").unwrap_or_default();
            let device_name = sysfs_read(&path, "label")
                .unwrap_or_else(|| format!("NVIDIA PCI Device {device_id}"));

            Some(PciGpu { slot, device_name })
        })
        .collect()
}

fn main() {
    println!("=== nvidia-probe ===\n");

    // Try NVML first (full runtime info)
    if let Some(nvml) = Nvml::load() {
        let gpus = nvml.query_gpus();
        if gpus.is_empty() {
            println!("[NVML] Library loaded but no GPUs found\n");
        } else {
            println!("[NVML] {} GPU(s) detected:\n", gpus.len());
            for gpu in &gpus {
                println!("  GPU {}: {}", gpu.index, gpu.name);
                if let Some((total, used, free)) = gpu.memory {
                    println!(
                        "    Memory: {} used / {} free / {} total",
                        format_bytes(used),
                        format_bytes(free),
                        format_bytes(total),
                    );
                }
                if let Some(temp) = gpu.temperature {
                    println!("    Temp:   {temp} C");
                }
                if let Some((gpu_util, mem_util)) = gpu.utilization {
                    println!("    Usage:  GPU {gpu_util}% | Memory {mem_util}%");
                }
                println!();
            }
            return;
        }
    } else {
        println!("[NVML] libnvidia-ml.so not available\n");
    }

    // Fallback: PCI sysfs scan
    let pci_gpus = scan_pci_gpus();
    if pci_gpus.is_empty() {
        println!("[PCI]  No NVIDIA GPUs found\n");
        println!("NVIDIA GPU: NOT DETECTED");
        std::process::exit(1);
    } else {
        println!("[PCI]  {} NVIDIA GPU(s) found:\n", pci_gpus.len());
        for gpu in &pci_gpus {
            println!("  {} (slot {})", gpu.device_name, gpu.slot);
        }
        println!();
        println!("NVIDIA GPU: DETECTED (driver not loaded — limited info via PCI)");
    }
}
