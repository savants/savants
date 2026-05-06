{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  buildInputs = with pkgs; [
    elfutils
    zlib
    pkg-config
  ];
}
