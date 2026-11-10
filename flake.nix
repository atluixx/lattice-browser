{
  description = "Lattice M0 NixOS dev env";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    fenix = { url = "github:nix-community/fenix"; inputs.nixpkgs.follows = "nixpkgs"; };
  };
  outputs = { self, nixpkgs, fenix }: let
    system = "x86_64-linux";
    pkgs = import nixpkgs { inherit system; };
    toolchain = fenix.packages.${system}.stable.toolchain;
  in {
    devShells.${system}.default = pkgs.mkShell {
      packages = with pkgs; [
        toolchain
        pkg-config
        rustPlatform.bindgenHook
        llvmPackages.libstdcxxClang
        cmake
        llvm
        m4
        perl
        python311
        git
        mold
        wayland
        wayland-protocols
        libxkbcommon
        libx11
        libxcb
        openssl
        fontconfig
        freetype
        harfbuzz
        libunwind
        zlib
        ccache
        time
      ];
      env = {
        NIX_CFLAGS_COMPILE = "-Wno-error=format-security";
      };
      # tikv-jemalloc-sys 0.6.1 (via servo 0.1.4) configure fails when the
      # wrapper injects -D_FORTIFY_SOURCE with -O0: glibc 2.42 features.h
      # #warnings + configure's -Werror. M0 workaround, revisit on monthly Servo.
      hardeningDisable = [ "fortify" ];
      shellHook = ''
        echo "Lattice M0 shell: rust $(rustc --version), servo LTS 0.1.4 pinned (nixpkgs validates 0.7.0 path)"
      '';
    };
    packages.${system}.lattice-shell = pkgs.rustPlatform.buildRustPackage {
      pname = "lattice-shell";
      version = "0.0.1";
      src = ./.;
      cargoLock.lockFile = ./Cargo.lock;
    };
  };
}
