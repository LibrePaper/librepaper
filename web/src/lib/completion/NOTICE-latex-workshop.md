# Notice: LaTeX-Workshop data

The LaTeX command and environment completion in `latex-workshop.json` is
derived from two files of LaTeX-Workshop:

- `data/commands.json`
- `data/environments.json`

Source: https://github.com/James-Yu/LaTeX-Workshop
Commit: fb1d13108f1947028edb3d9881ed4843b983a3c8

`web/tools/vendor-latex-workshop.mjs` reads those two files at that commit
and writes the converted list. Nothing else from LaTeX-Workshop is used. In
particular, `data/packages` is not used, because it is generated from GPL
TeXstudio files.

The license of LaTeX-Workshop follows. It is taken from `LICENSE.txt` at the
commit above.


The MIT License (MIT)

Copyright (c) 2016 James Yu

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
