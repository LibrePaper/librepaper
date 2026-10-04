# LibrePaper brand artwork

The served files live in `web/public/assets/`, where Vite copies them into the
deployed application:

- `librepaper-icon.svg`: the icon, and its own source; edit it in place.
- `librepaper-logo.svg`: the wordmark, outlined into paths.

`librepaper-logo-source.svg` here is the wordmark's editable source, with the
word kept as live Gotham HTF text. To update the served logo, export it with
Inkscape's text-to-path and plain-SVG options, crop the view box with ten
units of space around the drawing, and commit the result as
`web/public/assets/librepaper-logo.svg`.
