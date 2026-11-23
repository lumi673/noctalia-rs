{
  pkgs,
  pname,
  version,
  src,
  cargoLock,
  helperName,
  package ? pname,
  buildInputs ? [],
  nativeBuildInputs ? [],
}:
pkgs.rustPlatform.buildRustPackage {
  inherit pname version src cargoLock buildInputs nativeBuildInputs;
  cargoBuildFlags = ["-p" package];

  postInstall = ''
    install -Dm755 "$out/bin/${package}" "$out/${helperName}"
    rm -f "$out/bin/${package}"
    "$out/${helperName}" emit "$out" "${helperName}"
  '';
}
