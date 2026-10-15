# Where rendering happens

| Client or service | Role | Trade-off |
| --- | --- | --- |
| Browser client | Edit, collaborate, and render with WebAssembly | No installation; bounded browser resources |
| Desktop app | Use installed Quarto, Calepin, or TeX tools | Full local toolchain; requires the desktop app |
| Server | Synchronize source and history | Stable shared state; performs no compilation or storage of rendered results |
