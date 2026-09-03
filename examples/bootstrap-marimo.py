import marimo

__generated_with = "0.8.0"
app = marimo.App()


@app.cell
def __():
    import numpy as np
    import matplotlib.pyplot as plt
    return np, plt


@app.cell
def __(np):
    # Generate sample data
    np.random.seed(42)
    data = np.random.normal(100, 15, 100)

    print(f"Sample mean: {data.mean():.2f}")
    print(f"Sample std: {data.std():.2f}")
    return data


@app.cell
def __():
    # Title
    return "# What the Bootstrap Actually Resamples"


@app.cell
def __():
    return "## The Bootstrap Procedure"


@app.cell
def __():
    return """The bootstrap is a resampling method that works by drawing samples with
replacement from the observed data. This allows us to estimate the sampling
distribution of a statistic without making strong parametric assumptions."""


@app.cell
def __(np, data):
    # Bootstrap procedure
    n_bootstrap = 10000
    bootstrap_means = []

    for _ in range(n_bootstrap):
        bootstrap_sample = np.random.choice(data, size=len(data), replace=True)
        bootstrap_means.append(bootstrap_sample.mean())

    bootstrap_means = np.array(bootstrap_means)

    # Calculate confidence interval
    ci_lower = np.percentile(bootstrap_means, 2.5)
    ci_upper = np.percentile(bootstrap_means, 97.5)

    print(f"95% Confidence Interval: [{ci_lower:.2f}, {ci_upper:.2f}]")
    return bootstrap_means, ci_lower, ci_upper


@app.cell
def __():
    return "## Key Insights"


@app.cell
def __():
    return """The bootstrap tells us about the sampling variability of our statistic.
It assumes that the observed sample is representative of the underlying population."""


if __name__ == "__main__":
    app.run()
