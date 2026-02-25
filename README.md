# niritiling

https://github.com/user-attachments/assets/dfd23b5a-aece-4a05-a637-86da40b40d32

niritiling is a simple automatic tiling utility for the first window in a workspace in [Niri](https://github.com/niri-wm/niri).

tl;dr: it makes sure that if there is only a single non-floating window in a workspace, that window will take up the whole space.

When a workspace has a single tiled (=non-floating) window, it is automatically maximized. When a second tiled window is opened in that workspace, the first reverts back to its previous width. When only one window remains in a workspace after closing another, that triggers maximization again. Floating windows are ignored in the count.

## Usage

### NixOS

Add niritiling to your flake.nix' inputs:

```nix
{
  inputs.niritiling.url = "github:Swarsel/niritiling";
}
```

Then, inside a module:

```nix
{ inputs, ... }:
{
  imports = [ inputs.niritiling.nixosModules.default ];
  config.services.niritiling.enable = true;
}
```

If you are not on flakes, I trust you know how to set this up :)

### Non-NixOS

1. Build it: `cargo build --release`
2. Setup the service:
```ini
[Unit]
After=graphical-session.target
Description=First-window tiling service for Niri
PartOf=graphical-session.target

[Service]
ExecStart=<niritiling path>
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
```
