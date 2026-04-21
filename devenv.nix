{ pkgs, lib, inputs, ... }:
let
  hugrenv = pkgs.callPackage ./hugrenv.nix {
    packages = ["llvm"];
  };
in {
  packages = [
    pkgs.zstd
    # For now we exclude this, as some LLVM tools (clang 19 on MacOS) get
    # confused by the presence of the 14.0 version.
    # pkgs.llvmPackages_14.libllvm

    # These are needed to link to libllvm
    pkgs.libffi
    pkgs.act
    pkgs.cargo-insta
  ]
  ++ lib.optionals pkgs.stdenv.isDarwin [
    pkgs.xz
  ];

  enterShell = ''
    # append hugrenv to bin and lib paths
    export PATH="${hugrenv}/bin:$PATH"
    # if macos use DYLD_LIBRARY_PATH instead of LD_LIBRARY_PATH
    if [ "$(uname)" = "Darwin" ]; then
      export DYLD_LIBRARY_PATH="${hugrenv}/lib:${hugrenv}/lib64:${pkgs.stdenv.cc.cc.lib}/lib:$DYLD_LIBRARY_PATH"
    else
      export LD_LIBRARY_PATH="${hugrenv}/lib:${hugrenv}/lib64:${pkgs.stdenv.cc.cc.lib}/lib:$LD_LIBRARY_PATH"
    fi
  '';

  env = {
    LD_LIBRARY_PATH = lib.optionalString pkgs.stdenv.isLinux (lib.makeLibraryPath [ pkgs.stdenv.cc.cc.lib ]);
    HUGRENV_PATH = "${hugrenv}";
    LLVM_SYS_211_PREFIX = "${hugrenv}";
    LIBCLANG_PATH = "${hugrenv}/lib";
  };

  languages.python = {
    enable = true;
    uv = {
      enable = true;
      sync.enable = true;
    };
    venv.enable = true;
  };

  languages.rust = {
    enable = true;
    channel = "stable";
    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" ];
  };
}
