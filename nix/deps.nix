{ pkgs, gtk3 ? true, gtk4 ? true, qt5 ? false, qt6 ? true }:
{
  kimeBuildInputs = with pkgs; [
    dbus
    libdbusmenu

    libxcb
    libGL
    wayland
    libxkbcommon
  ]
  ++ pkgs.lib.optional gtk3 pkgs.gtk3
  ++ pkgs.lib.optional gtk4 pkgs.gtk4
  ++ pkgs.lib.optional qt5 pkgs.qt5.qtbase
  ++ pkgs.lib.optional qt6 pkgs.qt6.qtbase;

  # kime-candidate-window reaches these through dlopen (winit/glutin load
  # libwayland, libEGL and libxkbcommon at runtime), so nothing links against
  # them and fixupPhase's RPATH shrinking drops them. They have to be put back
  # explicitly or the candidate window cannot open a window at all.
  candidateWindowRuntimeInputs = with pkgs; [
    wayland
    libxkbcommon
    libGL
    libX11
    libXcursor
    libXrandr
    libXi
  ];

  kimeNativeBuildInputs = with pkgs; [
    python3 # xcb 0.9.0
    pkg-config
    llvmPackages_18.clang
    llvmPackages_18.libclang.lib
    llvmPackages_18.bintools
    meson
    ninja
  ];
}

