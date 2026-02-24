#!/usr/bin/env python3
"""Generate 3D surface plots of the WorkPool memory-aware scaling objective.

Produces two SVG outputs:
  1. scaling_objective.svg  — Objective value J(N) surface
  2. scaling_threads.svg    — Steady-state thread count surface

Usage:
  python3 scaling_surface.py [--output-dir DIR] [--throughput-weight W]
                              [--queue-depth-weight W] [--memory-weight W]
                              [--rss-weight W] [--min-threads N] [--max-threads N]

Axes:
  X = memory pressure (0–3)
  Y = CPU efficiency signal (−w_tp×ema_tp + w_qd×ema_qd)
  Z = objective value or steady-state thread count

The CPU efficiency signal combines throughput and queue depth into a single
axis, allowing visualization of the full 4D objective on a 3D surface.
"""

import argparse
import numpy as np
import matplotlib
matplotlib.use('Agg')  # Non-interactive backend for SVG output
import matplotlib.pyplot as plt
from mpl_toolkits.mplot3d import Axes3D  # noqa: F401


def usl_throughput(N, T1=1000.0, sigma=0.05, kappa=0.005):
    """Universal Scalability Law: T(N) = T1 * N / (1 + sigma*(N-1) + kappa*N*(N-1))"""
    denom = 1 + sigma * (N - 1) + kappa * N * (N - 1)
    return T1 * N / denom


def objective(cpu_signal, mem_pressure, w_mp, w_rss, rss_ratio=0.5):
    """Compute objective value.

    J = cpu_signal + w_mp * mem_pressure + w_rss * rss_pressure

    where cpu_signal = -w_tp * ema_tp + w_qd * ema_qd (pre-computed)
    and rss_pressure = rss_ratio * mem_pressure (correlated with slab).
    """
    rss_pressure = np.clip(rss_ratio * mem_pressure, 0, 3)
    return cpu_signal + w_mp * mem_pressure + w_rss * rss_pressure


def simulate_hill_climber(cpu_signal, mem_pressure, w_tp, w_qd, w_mp, w_rss,
                          min_threads, max_threads, rss_ratio=0.5,
                          T1=1000.0, sigma=0.05, kappa=0.005):
    """Simulate hill climber convergence to find steady-state thread count.

    Starting from min_threads, iteratively adjust ±1 based on objective delta.
    Returns the thread count after convergence.
    """
    threshold = 0.05
    n = (min_threads + max_threads) // 2  # Start from midpoint
    cooldown = 5
    direction = 1
    prev_obj = None

    for _ in range(200):  # Max iterations
        # Compute throughput at current N
        tp = usl_throughput(n, T1, sigma, kappa)
        # Queue depth model: decreases with more threads, floor at 0
        qd = max(0.0, float(cpu_signal) + w_tp * tp) / w_qd if w_qd > 0 else 0
        qd = max(0, qd)

        # Compute memory signals
        rss_p = min(3.0, rss_ratio * float(mem_pressure))

        # Compute current objective
        obj = (-w_tp * tp + w_qd * qd
               + w_mp * float(mem_pressure) + w_rss * rss_p)

        if prev_obj is None:
            prev_obj = obj
            continue

        if cooldown > 0:
            cooldown -= 1
            prev_obj = obj
            continue

        improvement = prev_obj - obj
        prev_obj = obj

        if improvement >= threshold:
            # Continue in same direction
            pass
        elif improvement <= -threshold:
            # Reverse
            direction = -direction
        else:
            continue

        # Apply direction
        new_n = n + direction
        if new_n < min_threads or new_n > max_threads:
            direction = -direction
            continue

        n = new_n
        cooldown = 5

    return n


def plot_objective_surface(ax, mem_grid, cpu_grid, w_mp, w_rss, rss_ratio=0.5):
    """Plot the objective value surface."""
    Z = objective(cpu_grid, mem_grid, w_mp, w_rss, rss_ratio)

    surf = ax.plot_surface(mem_grid, cpu_grid, Z,
                           cmap='RdYlGn_r', alpha=0.85,
                           edgecolor='none', antialiased=True)
    ax.set_xlabel('Memory Pressure', fontsize=10, labelpad=8)
    ax.set_ylabel('CPU Signal\n(-w_tp*tp + w_qd*qd)', fontsize=9, labelpad=8)
    ax.set_zlabel('Objective J(N)', fontsize=10, labelpad=8)
    ax.set_title('Scaling Objective Landscape', fontsize=13, fontweight='bold', pad=15)
    return surf


def plot_thread_surface(ax, mem_grid, cpu_grid, w_tp, w_qd, w_mp, w_rss,
                        min_threads, max_threads, rss_ratio=0.5):
    """Plot the steady-state thread count surface."""
    Z = np.zeros_like(mem_grid)
    for i in range(mem_grid.shape[0]):
        for j in range(mem_grid.shape[1]):
            Z[i, j] = simulate_hill_climber(
                cpu_grid[i, j], mem_grid[i, j],
                w_tp, w_qd, w_mp, w_rss,
                min_threads, max_threads, rss_ratio
            )

    surf = ax.plot_surface(mem_grid, cpu_grid, Z,
                           cmap='viridis', alpha=0.85,
                           edgecolor='none', antialiased=True)
    ax.set_xlabel('Memory Pressure', fontsize=10, labelpad=8)
    ax.set_ylabel('CPU Signal\n(-w_tp*tp + w_qd*qd)', fontsize=9, labelpad=8)
    ax.set_zlabel('Steady-State Threads', fontsize=10, labelpad=8)
    ax.set_title('Steady-State Thread Count', fontsize=13, fontweight='bold', pad=15)
    return surf


def main():
    parser = argparse.ArgumentParser(
        description='Generate 3D surface plots for WorkPool memory-aware scaling'
    )
    parser.add_argument('--output-dir', default='.',
                        help='Output directory for SVG files')
    parser.add_argument('--throughput-weight', type=float, default=1.0,
                        help='Throughput weight (w_tp)')
    parser.add_argument('--queue-depth-weight', type=float, default=0.5,
                        help='Queue depth weight (w_qd)')
    parser.add_argument('--memory-weight', type=float, default=5.0,
                        help='Memory pressure weight (w_mp)')
    parser.add_argument('--rss-weight', type=float, default=8.0,
                        help='RSS pressure weight (w_rss)')
    parser.add_argument('--min-threads', type=int, default=1,
                        help='Minimum thread count')
    parser.add_argument('--max-threads', type=int, default=18,
                        help='Maximum thread count')
    parser.add_argument('--grid-resolution', type=int, default=50,
                        help='Grid resolution for surface plots')
    args = parser.parse_args()

    w_tp = args.throughput_weight
    w_qd = args.queue_depth_weight
    w_mp = args.memory_weight
    w_rss = args.rss_weight
    min_threads = args.min_threads
    max_threads = args.max_threads
    res = args.grid_resolution

    # Create grids
    mem_range = np.linspace(0, 3, res)
    cpu_range = np.linspace(-200, 50, res)  # CPU signal range
    mem_grid, cpu_grid = np.meshgrid(mem_range, cpu_range)

    # --- Plot 1: Objective Surface ---
    fig1 = plt.figure(figsize=(10, 7))
    ax1 = fig1.add_subplot(111, projection='3d')
    surf1 = plot_objective_surface(ax1, mem_grid, cpu_grid, w_mp, w_rss)
    fig1.colorbar(surf1, ax=ax1, shrink=0.5, aspect=10, label='Objective Value')

    # Add weight annotation
    weight_text = (f'Weights: w_tp={w_tp}, w_qd={w_qd}, '
                   f'w_mp={w_mp}, w_rss={w_rss}')
    fig1.text(0.5, 0.02, weight_text, ha='center', fontsize=8, style='italic')

    ax1.view_init(elev=25, azim=-45)
    fig1.tight_layout()
    path1 = f'{args.output_dir}/scaling_objective.svg'
    fig1.savefig(path1, format='svg', bbox_inches='tight', dpi=150)
    print(f'Saved: {path1}')
    plt.close(fig1)

    # --- Plot 2: Thread Count Surface ---
    fig2 = plt.figure(figsize=(10, 7))
    ax2 = fig2.add_subplot(111, projection='3d')
    surf2 = plot_thread_surface(ax2, mem_grid, cpu_grid,
                                w_tp, w_qd, w_mp, w_rss,
                                min_threads, max_threads)
    fig2.colorbar(surf2, ax=ax2, shrink=0.5, aspect=10, label='Thread Count')

    config_text = (f'Range: [{min_threads}, {max_threads}] threads | '
                   f'Weights: w_tp={w_tp}, w_qd={w_qd}, '
                   f'w_mp={w_mp}, w_rss={w_rss}')
    fig2.text(0.5, 0.02, config_text, ha='center', fontsize=8, style='italic')

    ax2.view_init(elev=25, azim=-45)
    fig2.tight_layout()
    path2 = f'{args.output_dir}/scaling_threads.svg'
    fig2.savefig(path2, format='svg', bbox_inches='tight', dpi=150)
    print(f'Saved: {path2}')
    plt.close(fig2)


if __name__ == '__main__':
    main()
