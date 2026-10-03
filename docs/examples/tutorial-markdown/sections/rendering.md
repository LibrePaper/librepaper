# Where rendering happens

| Client or service | Role | Trade-off |
| --- | --- | --- |
| Browser client | Edit, collaborate, and render with WebAssembly | No installation; bounded browser resources |
| Local companion | Use installed Quarto, Calepin, or TeX tools | Full local toolchain; requires pairing |
| Server | Synchronize source and history | Stable shared state; performs no compilation or storage of rendered results |
