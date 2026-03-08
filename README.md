# nvidia-probe

A tiny tool that checks if NVIDIA GPU is present on a machine and if drivers are installed and loaded.

## Quick start

```sh
podman run --rm --privileged -v /:/host:ro ghcr.io/shadi/nvidia-probe:latest
```

#### Machine does not have an nvidia card:
```
$ podman run --rm --privileged -v /:/host:ro ghcr.io/shadi/nvidia-probe:latest
=== nvidia-probe ===

[NVML] libnvidia-ml.so not available

[PCI]  No NVIDIA GPUs found

NVIDIA GPU: NOT DETECTED

```

#### Machine has nvidia card but the driver is not installed
```
❯ podman run --rm --privileged -v /:/host:ro ghcr.io/shadi/nvidia-probe:latest
=== nvidia-probe ===

[NVML] libnvidia-ml.so not available

[PCI]  1 NVIDIA GPU(s) found:

  NVIDIA PCI Device 0x2786 (slot 0000:01:00.0)

NVIDIA GPU: DETECTED (driver not loaded — limited info via PCI)
```
#### Machine has nvidia card and driver is installed.
```
❯ podman run --rm --privileged -v /:/host:ro ghcr.io/shadi/nvidia-probe:latest 
=== nvidia-probe ===

[NVML] 1 GPU(s) detected:

  GPU 0: NVIDIA GeForce RTX 4070
    Memory: 1.2 GiB used / 10.8 GiB free / 12.0 GiB total
    Temp:   34 C
    Usage:  GPU 7% | Memory 6%

```

## On Kubernetes

A Job manifest is provided at `k8s/nvidia-probe-privileged.yaml`. Before applying, edit it to match your cluster:

1. Set `nodeSelector` to target your GPU nodes (default is `cloud.google.com/gke-nodepool: gpu-node-pool`)
2. Set `tolerations` to match the taints on your GPU nodes (default is `nvidia.com/gpu=present:NoSchedule`)

Then run:

```sh
kubectl apply -f k8s/nvidia-probe-privileged.yaml
kubectl logs job/nvidia-probe
```

Clean up when done:

```sh
kubectl delete job nvidia-probe
```

## How it works

When you install NVIDIA drivers on Linux, they come with a library called [NVML](https://developer.nvidia.com/management-library-nvml) (NVIDIA Management Library). This tool try to call the interface of NVML to find information about the GPU.

nvidia-probe tries to load this library and talk to your GPU. If it can, it shows you:

- GPU name
- Memory usage
- Temperature
- GPU utilization

If the NVIDIA driver isn't installed, it falls back to scanning your PCI devices to check if an NVIDIA GPU physically exists in your machine.
