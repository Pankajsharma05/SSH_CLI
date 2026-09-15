"""SSH_CLI matplotlib backend — makes plt.show() work over SSH.

A cluster login node usually has no display, so `plt.show()` either errors or
does nothing. This backend renders each open figure to a PNG in a directory
that the SSH_CLI app watches; the app then pops the figure up in a viewer tab,
which is as close to local `plt.show()` behaviour as a remote shell can get.

Activated with an environment variable, so scripts need no changes:

    export PYTHONPATH="$HOME/.ssh_cli:$PYTHONPATH"
    export MPLBACKEND="module://ssh_cli_mpl"

Output directory: $SSH_CLI_PLOT_DIR, else ~/.ssh_cli/plots

Deliberately written against matplotlib's public API only (no `_Backend`), so
it keeps working across the range of matplotlib versions found on clusters.
`show()` does not block — the script keeps running, which is what you want
when the figure is displayed by a separate application.
"""

import os
import sys
import time

from matplotlib.backend_bases import FigureManagerBase
from matplotlib.backends.backend_agg import FigureCanvasAgg

backend_version = "ssh_cli"

# matplotlib looks these up on the backend module.
FigureCanvas = FigureCanvasAgg
FigureManager = FigureManagerBase

# Monotonic within the process. Figure numbers restart at 1 after
# plt.close("all"), and the timestamp only has second resolution, so
# without this a second show() in the same second would overwrite the
# first one's PNG and the figure would be lost.
_seq = 0

# Interactive redraws go to one reused file; this bounds how often it is
# rewritten so a tight plt.pause() loop cannot saturate the link.
LIVE_MIN_INTERVAL = 0.25
_last_live = 0.0


def _outdir():
    d = os.environ.get("SSH_CLI_PLOT_DIR") or os.path.join(
        os.path.expanduser("~"), ".ssh_cli", "plots"
    )
    try:
        os.makedirs(d, exist_ok=True)
    except OSError:
        pass
    return d


def _save(fig, name=None):
    """Write one figure atomically, so the watcher never reads a half file.

    With no name, a unique one is generated: a new figure, i.e. a new tab in
    the app. Passing a fixed name overwrites in place instead, which the app
    shows as one auto-refreshing tab (see draw_if_interactive).
    """
    global _seq
    d = _outdir()
    if name is None:
        _seq += 1
        stamp = time.strftime("%Y%m%d-%H%M%S")
        name = "fig-{0}-{1}-{2:03d}.png".format(stamp, os.getpid(), _seq)
    final = os.path.join(d, name)
    tmp = os.path.join(d, "." + name + ".part")
    try:
        fig.savefig(tmp, format="png", dpi=fig.get_dpi(), facecolor=fig.get_facecolor())
        os.replace(tmp, final)
    except Exception as exc:                                  # noqa: BLE001
        sys.stderr.write("ssh_cli backend: could not save figure: {0}\n".format(exc))
        try:
            os.remove(tmp)
        except OSError:
            pass
        return None
    return final


def show(*args, **kwargs):
    """Render every open figure to the watch directory.

    Two different meanings, mirroring what a local window manager does:

    * normal script  - show() means "display these figures". Each gets its
      own file, so each pops up as a new tab, and the figures are closed.
    * interactive    - plt.ion()/plt.pause() call show() on every frame to
      refresh an existing window. Those reuse one file per figure number so
      the app updates a single tab, and figures are NOT closed (closing them
      would destroy the animation the caller is driving).
    """
    global _last_live
    import matplotlib
    import matplotlib.pyplot as plt

    if matplotlib.is_interactive():
        now = time.time()
        if now - _last_live < LIVE_MIN_INTERVAL:
            return []
        _last_live = now
        live = []
        for num in plt.get_fignums():
            path = _save(plt.figure(num), name="live-{0}.png".format(num))
            if path:
                live.append(path)
        return live                      # no close, no per-frame chatter

    written = []
    for num in plt.get_fignums():
        path = _save(plt.figure(num))
        if path:
            written.append(path)
    plt.close("all")
    if written:
        sys.stdout.write(
            "[SSH_CLI] {0} figure(s) sent to the app: {1}\n".format(
                len(written), ", ".join(os.path.basename(p) for p in written)
            )
        )
        sys.stdout.flush()
    return written


def draw_if_interactive():
    """Interactive mode (plt.ion) only: keep one live-updating figure.

    Guarded two ways. Without the is_interactive() check this also fires
    during ordinary non-interactive scripts, duplicating every figure. And
    writing a *fixed* filename (rather than a unique one) means an animation
    loop calling plt.pause() updates a single auto-refreshing tab instead of
    flooding the app with thousands of PNGs and tabs.
    """
    global _last_live
    import matplotlib
    import matplotlib.pyplot as plt

    if not matplotlib.is_interactive():
        return
    now = time.time()
    if now - _last_live < LIVE_MIN_INTERVAL:
        return
    nums = plt.get_fignums()
    if not nums:
        return
    _last_live = now
    for num in nums:
        _save(plt.figure(num), name="live-{0}.png".format(num))


def new_figure_manager(num, *args, **kwargs):
    """Create a figure and wrap it in a manager (matplotlib's entry point)."""
    from matplotlib.figure import Figure

    figure_class = kwargs.pop("FigureClass", Figure)
    return new_figure_manager_given_figure(num, figure_class(*args, **kwargs))


def new_figure_manager_given_figure(num, figure):
    return FigureManagerBase(FigureCanvasAgg(figure), num)
