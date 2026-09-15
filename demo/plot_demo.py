#!/usr/bin/env python3
"""Demo for SSH_CLI's image viewer.

Run this (locally or on a server), then double-click plot_demo.png in the
file pane. Re-run it and the open viewer tab updates by itself.
"""
import os
import numpy as np
import matplotlib
matplotlib.use("Agg")            # no display needed — we save to a file
import matplotlib.pyplot as plt

# save next to this script, so the output is where you expect it
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "plot_demo.png")

x = np.linspace(0, 4 * np.pi, 500)
phase = np.random.rand() * 2 * np.pi      # changes every run, so refresh is visible

fig, ax = plt.subplots(figsize=(8, 4.5), dpi=140)
ax.plot(x, np.sin(x + phase), label="sin(x + φ)", lw=2)
ax.plot(x, np.cos(x + phase), label="cos(x + φ)", lw=2, ls="--")
ax.set_title(f"SSH_CLI image viewer demo — φ = {phase:.2f} rad")
ax.set_xlabel("x")
ax.set_ylabel("amplitude")
ax.legend()
ax.grid(alpha=.3)
fig.tight_layout()
fig.savefig(out)
print("wrote", out)
