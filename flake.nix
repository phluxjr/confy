{
  description = "a config manager for linux/unix systems";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }: let
    systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
  in {
    packages = forAllSystems (pkgs: {
      default = pkgs.python3Packages.buildPythonApplication {
        pname = "confy-tui";
        version = "2.1.2";
        src = ./.;
        format = "pyproject";
        nativeBuildInputs = [ pkgs.python3Packages.hatchling ];
        meta = {
          description = "a config manager for linux/unix systems";
          license = pkgs.lib.licenses.gpl3Plus;
          mainProgram = "confy";
        };
      };
    });
  };
}
