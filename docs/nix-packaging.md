# Optional Nix packaging

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    noctalia-rs.url = "github:lumi673/noctalia-rs";
  };

  outputs = { self, nixpkgs, flake-utils, noctalia-rs }:
    flake-utils.lib.eachDefaultSystem (system: {
      packages.default = noctalia-rs.mkPlugin.${system} {
        pname = "my-plugin";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        package = "my-plugin";
        helperName = "my-helper";
      };
    });
}
```

Run `nix build --out-link dist`. The output is a Noctalia local plugin source.
Nix only automates the build and helper placement. It is not required by the
library.
