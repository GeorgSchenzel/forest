# The forest CLI.
{
  lib,
  rustPlatform,
  makeWrapper,
  gitMinimal,
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

  # worktree and branch operations go through git; gitMinimal keeps python,
  # docs and the toolchain out of the closure, and --suffix lets a git already
  # on PATH (with the user's credential helpers etc.) win
  postInstall = ''
    wrapProgram $out/bin/forest \
      --suffix PATH : ${lib.makeBinPath [ gitMinimal ]}
  '';

  meta = {
    description = "Worktree & forest manager";
    mainProgram = "forest";
    platforms = lib.platforms.unix;
  };
}
