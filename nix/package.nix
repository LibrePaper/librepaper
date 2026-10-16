{
  lib,
  stdenv,
  fetchurl,
  dpkg,
  autoPatchelfHook,
  wrapGAppsHook3,
  webkitgtk_4_1,
  gtk3,
  libsoup_3,
  glib,
  glib-networking,
  gdk-pixbuf,
  cairo,
  pango,
  harfbuzz,
  atk,
  dbus,
  openssl,
  ...
}:

let
  source = lib.importJSON ./source.json;
in
stdenv.mkDerivation {
  pname = "librepaper";
  inherit (source) version;

  src = fetchurl {
    inherit (source) url hash;
  };

  nativeBuildInputs = [
    dpkg
    autoPatchelfHook
    wrapGAppsHook3
  ];

  # glib-networking gives WebKit its TLS backend.
  buildInputs = [
    webkitgtk_4_1
    gtk3
    libsoup_3
    glib
    glib-networking
    gdk-pixbuf
    cairo
    pango
    harfbuzz
    atk
    dbus
    openssl
    stdenv.cc.cc.lib
  ];

  dontConfigure = true;
  dontBuild = true;

  unpackPhase = ''
    runHook preUnpack
    dpkg-deb -x $src .
    runHook postUnpack
  '';

  installPhase = ''
    runHook preInstall

    mkdir -p $out
    cp -r usr/bin usr/share $out/

    # Start the desktop app from the store path, not from a bare name.
    find $out/share/applications -name '*.desktop' -exec \
      substituteInPlace {} --replace-fail \
        "Exec=librepaper-desktop" "Exec=$out/bin/librepaper-desktop" \;

    runHook postInstall
  '';

  meta = {
    description = "LibrePaper desktop app, with the librepaper command line";
    homepage = "https://librepaper.org";
    license = lib.licenses.mit;
    platforms = [ "x86_64-linux" ];
    mainProgram = "librepaper-desktop";
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
