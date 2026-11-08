@echo off
REM DMZ Toolchain Environment Activator (Windows)
set "DMZ_WORKSPACE=%~dp0.."
if exist "%~dp0..\dmz-workspace.toml" (
    set "DMZ_WORKSPACE=%~dp0.."
) else if exist "%~dp0..\..\dmz-workspace.toml" (
    for %%I in ("%~dp0..\..") do set "DMZ_WORKSPACE=%%~fI"
) else if exist "%~dp0..\..\..\dmz-workspace.toml" (
    for %%I in ("%~dp0..\..\..") do set "DMZ_WORKSPACE=%%~fI"
) else if exist "%~dp0..\..\.dmz\toolchains" (
    for %%I in ("%~dp0..\..") do set "DMZ_WORKSPACE=%%~fI"
) else if exist "%~dp0..\..\..\.dmz\toolchains" (
    for %%I in ("%~dp0..\..\..") do set "DMZ_WORKSPACE=%%~fI"
)

for /d %%D in ("%DMZ_WORKSPACE%\.dmz\toolchains\*") do (
    if exist "%%D\bin" (
        echo ;%PATH%; | findstr /I /C:";%%D\bin;" >nul || set "PATH=%%D\bin;%PATH%"
    )
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\rust" (
    set "RUSTUP_HOME=%DMZ_WORKSPACE%\.dmz\toolchains\rust"
    if not defined CARGO_HOME set "CARGO_HOME=%DMZ_WORKSPACE%\.cargo"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\go\bin\go.exe" (
    set "GOROOT=%DMZ_WORKSPACE%\.dmz\toolchains\go"
)

:: Explicitly clear PYTHONHOME to avoid host Python stdlib corruption ("failed to import encodings module")
set "PYTHONHOME="

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\node\lib\node_modules" (
    set "NODE_PATH=%DMZ_WORKSPACE%\.dmz\toolchains\node\lib\node_modules;%NODE_PATH%"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\php\bin\php.exe" (
    set "PHPRC=%DMZ_WORKSPACE%\.dmz\toolchains\php"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\ruby\bin\ruby.exe" (
    set "GEM_HOME=%DMZ_WORKSPACE%\.dmz\toolchains\ruby"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\clang.exe" (
    set "CC=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\clang.exe"
    set "CXX=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\clang++.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\gcc.exe" (
    set "CC=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\gcc.exe"
    set "CXX=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\g++.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\cc.exe" (
    set "CC=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\cc.exe"
)
if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ar.exe" (
    set "AR=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ar.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ar.exe" (
    set "AR=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ar.exe"
)
if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ranlib.exe" (
    set "RANLIB=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ranlib.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ranlib.exe" (
    set "RANLIB=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ranlib.exe"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cmake\bin\cmake.exe" (
    set "CMAKE=%DMZ_WORKSPACE%\.dmz\toolchains\cmake\bin\cmake.exe"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake" (
    if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cmake" (
        set "CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "HOST_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "TARGET_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
    ) else if exist "%DMZ_WORKSPACE%\CMakeLists.txt" (
        set "CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "HOST_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "TARGET_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
    )
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\ninja\bin\ninja.exe" (
    set "NINJA=%DMZ_WORKSPACE%\.dmz\toolchains\ninja\bin\ninja.exe"
    set "CMAKE_GENERATOR=Ninja"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\make.exe" (
    set "MAKE=%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\make.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\mingw32-make.exe" (
    set "MAKE=%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\mingw32-make.exe"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\juce" (
    set "JUCE_PATH=%DMZ_WORKSPACE%\.dmz\toolchains\juce"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\androidndk" (
    set "ANDROID_NDK_HOME=%DMZ_WORKSPACE%\.dmz\toolchains\androidndk"
    set "ANDROID_NDK_ROOT=%DMZ_WORKSPACE%\.dmz\toolchains\androidndk"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkg-config.exe" (
    set "PKG_CONFIG=%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkg-config.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkgconf.exe" (
    set "PKG_CONFIG=%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkgconf.exe"
)
if exist "%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\lib\pkgconfig" (
    set "PKG_CONFIG_PATH=%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\lib\pkgconfig;%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\share\pkgconfig;%PKG_CONFIG_PATH%"
)

echo DMZ bundled toolchains activated.
