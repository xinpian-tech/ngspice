{
  description = "ICsprout ngspice";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          ngspice = pkgs.stdenv.mkDerivation {
            pname = "ngspice-icsprout";
            version = "47";
            src = self;

            nativeBuildInputs = with pkgs; [
              autoreconfHook
              bison
              flex
              pkg-config
            ];

            configureFlags = [
              "--disable-debug"
              "--enable-xspice"
              "--enable-osdi"
              "--with-readline=no"
            ];

            enableParallelBuilding = true;

            meta = {
              description = "ngspice with ICsprout model support";
              homepage = "https://ngspice.sourceforge.io/";
              license = pkgs.lib.licenses.bsd3;
              mainProgram = "ngspice";
              platforms = systems;
            };
          };
        in {
          inherit ngspice;
          default = ngspice;
        });
    };
}
