# The forest CLI.
{
  lib,
  rustPlatform,
  makeWrapper,
  git,
}:
rustPlatform.buildRustPackage {
  pname = "forest";
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  # integration tests shell out to git; `just test` runs them
  doCheck = false;

  nativeBuildInputs = [ makeWrapper ];

  # worktree and branch operations go through git
  postInstall = ''
    wrapProgram $out/bin/forest \
      --prefix PATH : ${lib.makeBinPath [ git ]}
  '';

  meta = {
    description = "Worktree & forest manager";
    mainProgram = "forest";
    platforms = lib.platforms.unix;
  };
}
