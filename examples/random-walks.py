import marimo

__generated_with = "0.9.0"
app = marimo.App(width="medium")


@app.cell(hide_code=True)
def _(mo):
    mo.md(
        r"""
        # How Far Does a Drunk Walk?

        A random walk takes steps of $\pm 1$, each direction equally likely, each
        independent of the last. After $n$ steps its position is

        $$S_n = \sum_{i=1}^{n} X_i, \qquad X_i = \begin{cases} +1 & \text{with probability } 1/2 \\ -1 & \text{with probability } 1/2 \end{cases}$$

        The mean is zero by symmetry. That fact is true and almost useless, because
        it describes where the walk is on average, not how far from home it tends
        to be.
        """
    )
    return


@app.cell
def _():
    import marimo as mo
    import numpy as np
    import matplotlib.pyplot as plt

    rng = np.random.default_rng(20260903)
    return mo, np, plt, rng


@app.cell(hide_code=True)
def _(mo):
    mo.md(
        r"""
        ## The right question

        The quantity that carries the information is the variance:

        $$\operatorname{Var}(S_n) = \sum_{i=1}^{n} \operatorname{Var}(X_i) = n$$

        so the typical distance from the origin grows like $\sqrt{n}$, not like
        $n$. Four times as many steps take you only twice as far.
        """
    )
    return


@app.cell
def _(np, rng):
    def walk(n, size=None):
        """One random walk of n steps, or `size` of them stacked as rows."""
        shape = (n,) if size is None else (size, n)
        return np.cumsum(rng.choice([-1, 1], size=shape), axis=-1)

    def mean_distance(n, reps=2000):
        return np.abs(walk(n, size=reps)[:, -1]).mean()

    steps = np.array([25, 100, 400, 1600, 6400])
    distances = np.array([mean_distance(n) for n in steps])

    import pandas  # noqa: F401  (marimo renders a DataFrame as a table)
    return distances, mean_distance, pandas, steps, walk


@app.cell
def _(distances, np, pandas, steps):
    pandas.DataFrame(
        {
            "n": steps,
            "mean_dist": distances.round(2),
            "sqrt_n": np.sqrt(steps).round(2),
            "ratio": (distances / np.sqrt(steps)).round(3),
        }
    )
    return


@app.cell(hide_code=True)
def _(mo):
    mo.md(
        r"""
        The ratio settles near $\sqrt{2/\pi} \approx 0.798$, which is the mean of a
        half-normal distribution and exactly what the central limit theorem
        predicts for $E|S_n| / \sqrt{n}$.

        ## Twenty walks at once
        """
    )
    return


@app.cell
def _(np, plt, walk):
    n_steps = 500
    figure_paths, axis_paths = plt.subplots(figsize=(7, 4))
    for _path in walk(n_steps, size=20):
        axis_paths.plot(np.arange(n_steps + 1), np.concatenate([[0], _path]),
                        color="#3757d5", alpha=0.2, linewidth=1.5)
    _grid = np.linspace(0, n_steps, 400)
    axis_paths.plot(_grid, np.sqrt(_grid), color="#c0392b", linewidth=2)
    axis_paths.plot(_grid, -np.sqrt(_grid), color="#c0392b", linewidth=2)
    axis_paths.set(xlabel="steps", ylabel="position", ylim=(-70, 70))
    figure_paths
    return axis_paths, figure_paths, n_steps


@app.cell(hide_code=True)
def _(mo):
    mo.md(
        r"""
        The envelope is $\pm\sqrt{n}$. Most paths stay inside it most of the time,
        and the ones that wander outside are not anomalies: they are the tail the
        $\sqrt{n}$ scale is a summary of.

        ## The arcsine law

        Here is the result that makes random walks worth teaching. Ask what
        fraction of its time a walk spends on the positive side. Intuition says a
        half, with most walks near a half. Intuition is wrong, and not slightly.
        """
    )
    return


@app.cell
def _(np, plt, walk):
    fractions = (walk(1000, size=4000) > 0).mean(axis=1)

    figure_arcsine, axis_arcsine = plt.subplots(figsize=(7, 4))
    axis_arcsine.hist(fractions, bins=40, density=True, color="#3757d5",
                      alpha=0.2)
    _x = np.linspace(0.001, 0.999, 500)
    axis_arcsine.plot(_x, 1 / (np.pi * np.sqrt(_x * (1 - _x))),
                      color="#c0392b", linewidth=2)
    axis_arcsine.set(xlabel="fraction of time spent above zero", ylabel="density")
    figure_arcsine
    return axis_arcsine, figure_arcsine, fractions


@app.cell(hide_code=True)
def _(mo):
    mo.md(
        r"""
        The density is U-shaped: $f(x) = 1/(\pi\sqrt{x(1-x)})$. The _least_ likely
        outcome is an even split. The most likely outcomes are that the walk spends
        almost all of its time on one side.

        The reason is that a walk which drifts positive early has to return to zero
        before it can accumulate negative time, and returns to zero become rarer as
        the walk wanders. Leads are sticky. In a season of coin flips, one team
        leading throughout is not evidence of anything.

        ## Recurrence, and its price

        In one dimension the walk returns to the origin with probability one. It
        also takes its time about it: the expected waiting time is infinite. Both
        statements hold at once, and the second is why simulating the first is
        awkward.
        """
    )
    return


@app.cell
def _(np, walk):
    def return_times(reps=3000, cap=10000):
        paths = walk(cap, size=reps)
        hit = paths == 0
        ever = hit.any(axis=1)
        first = np.where(ever, hit.argmax(axis=1) + 1, -1).astype(float)
        first[~ever] = np.nan
        return first

    times = return_times()
    {
        "returned_within_cap": float(np.mean(~np.isnan(times))),
        "median_time": float(np.nanmedian(times)),
        "mean_time": float(np.nanmean(times)),
    }
    return return_times, times


@app.cell(hide_code=True)
def _(mo):
    mo.md(
        r"""
        The median return is quick, a handful of steps. The mean is enormous and
        grows with whatever cap you impose, because the distribution has tail
        $P(T > t) \sim \sqrt{2/(\pi t)}$ and no finite first moment. Reporting the
        mean of this sample would be reporting a property of the cap.
        """
    )
    return


if __name__ == "__main__":
    app.run()
