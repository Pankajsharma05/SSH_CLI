#!/usr/bin/env python3
"""Demo: plt.show() with no savefig — the cluster case.

With the `plots` button ON and this run from a terminal tab in the app,
the figure pops up in a viewer tab even though the host has no display.
"""
import numpy as np
import matplotlib.pyplot as plt

x = np.linspace(0, 4 * np.pi, 400)
phase = np.random.rand() * 2 * np.pi

plt.figure(figsize=(8, 4.5))
plt.plot(x, np.sin(x + phase), lw=2, label="sin(x + φ)")
plt.plot(x, np.cos(x + phase), lw=2, ls="--", label="cos(x + φ)")
plt.title(f"plt.show() over SSH — φ = {phase:.2f} rad")
plt.legend()
plt.grid(alpha=.3)
plt.tight_layout()

plt.show()          # <- no savefig anywhere
print("script finished")
