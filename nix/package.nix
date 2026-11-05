{ lib, rustPlatform }:

let
  cargo = lib.importTOML ../Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = cargo.package.name;
  version = cargo.package.version;

  # Only what the build needs: local databases, the image cache and build output stay out.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [ ../Cargo.toml ../Cargo.lock ../src ../web ../tests ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  meta = {
    description = "At-a-glance meal picker and diet history for Nice To Fit You (ntfy.pl)";
    homepage = "https://github.com/OmoYauhen/ntfy-dashboard";
    license = lib.licenses.mit;
    mainProgram = "ntfy-dashboard";
    platforms = lib.platforms.linux;
  };
}
