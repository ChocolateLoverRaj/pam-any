{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      flake-utils,
      crane,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        craneLib = crane.mkLib pkgs;
        pam-any = craneLib.buildPackage {
          src = craneLib.cleanCargoSource ./.;
          buildInputs = with pkgs; [
            rustPlatform.bindgenHook
            pam
          ];
        };
        vm = nixpkgs.lib.nixosSystem {
          inherit system;
          modules = [
            (
              { pkgs, ... }:
              {
                # Username is `a`
                users.users.a = {
                  isNormalUser = true;
                  # Password is `a`
                  hashedPassword = "$y$j9T$WRtR46dlzBxc2b3vYcNb..$sDQgS87Q2meiJUz9VA/akPL0uL1Uv1GFdG1P2ZANky/";
                  extraGroups = [ "wheel" ];
                };
                services.getty = {
                  autologinUser = "a";
                };
                security.pam.services = {
                  success2s = {
                    enable = true;
                    text = ''
                      auth required pam_exec.so quiet ${pkgs.coreutils}/bin/sleep 2
                    '';
                  };
                  fail2s = {
                    enable = true;
                    text = ''
                      auth required pam_exec.so quiet ${pkgs.coreutils}/bin/sleep 2
                      auth required pam_deny.so
                    '';
                  };
                  password-or-success2s = {
                    enable = true;
                    text = ''
                      auth sufficient ${pam-any}/lib/libpam_any.so { "mode": "One", "modules": { "login": "Password", "success2s": "Success in 2s" } }
                    '';
                  };
                  password-or-fail2s = {
                    enable = true;
                    text = ''
                      auth sufficient ${pam-any}/lib/libpam_any.so { "mode": "One", "modules": { "login": "Password", "fail2s": "Fails in 2s" } }
                    '';
                  };
                  password-and-success2s = {
                    enable = true;
                    text = ''
                      auth sufficient ${pam-any}/lib/libpam_any.so { "mode": "All", "modules": { "login": "Password", "success2s": "Success in 2s" } }
                    '';
                  };
                  password-and-fail2s = {
                    enable = true;
                    text = ''
                      auth sufficient ${pam-any}/lib/libpam_any.so { "mode": "All", "modules": { "login": "Password", "fail2s": "Fails in 2s" } }
                    '';
                  };
                };
                environment.systemPackages = with pkgs; [
                  pamtester
                  # (writeShellApplication {
                  #   name = "a";
                  #   text = ''
                  #     pamtester test "$USER" authenticate
                  #   '';
                  # })
                ];
                # environment.interactiveShellInit = "a";
                virtualisation.vmVariant.virtualisation = {
                  diskImage = null;
                };
              }
            )
          ];
        };
      in
      {
        devShells.default =
          with pkgs;
          mkShell {
            buildInputs = [
              rustPlatform.bindgenHook
              pam
              (rust-bin.stable.latest.default.override {
                extensions = [ "rust-src" ];
              })
            ];
          };
        nixosConfigurations.vm = vm;
        packages = {
          pam-any = pam-any;
          default = pam-any;
          vm = vm.config.system.build.vm;
        };
      }
    );
}
