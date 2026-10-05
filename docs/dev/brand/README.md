# LibrePaper brand artwork

The app serves artwork from [`web/public/assets/`](../../../web/public/assets/).
Edit `librepaper-icon.svg` there directly; it is its own source. The served
`librepaper-logo.svg` is the outlined wordmark. Its editable source is
[`librepaper-logo-source.svg`](librepaper-logo-source.svg), with live Gotham
HTF text.

To update the wordmark, export the source from Inkscape with text converted to
paths and plain SVG enabled. Crop the view box with ten units of space around
the drawing, then commit the result as
[`web/public/assets/librepaper-logo.svg`](../../../web/public/assets/librepaper-logo.svg).
