#!/bin/bash
set -euo pipefail

GODOT_VERSION="4.5.1"
GODOT_SUB="stable"
GODOT_DOWNLOAD_DIR="https://github.com/godotengine/godot-builds/releases/download/${GODOT_VERSION}-${GODOT_SUB}"
EXPORT_NAME="material_maker"
PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
GODOT_BIN="/Applications/Godot.app/Contents/MacOS/Godot"
TEMPLATES_DIR="$HOME/Library/Application Support/Godot/export_templates/${GODOT_VERSION}.${GODOT_SUB}"

cd "$PROJECT_DIR"

# --- Step 1: Download Godot if not installed ---
if [ ! -f "$GODOT_BIN" ]; then
    echo "==> Downloading Godot ${GODOT_VERSION}..."
    curl -L "${GODOT_DOWNLOAD_DIR}/Godot_v${GODOT_VERSION}-${GODOT_SUB}_macos.universal.zip" -o /tmp/godot_macos.zip
    unzip -o /tmp/godot_macos.zip -d /tmp
    cp -R /tmp/Godot.app /Applications/Godot.app
    xattr -dr com.apple.quarantine /Applications/Godot.app
    rm /tmp/godot_macos.zip
fi
echo "==> Using Godot: $("$GODOT_BIN" --version)"

# --- Step 2: Install export templates if missing ---
if [ ! -d "$TEMPLATES_DIR" ]; then
    echo "==> Downloading export templates..."
    curl -L "${GODOT_DOWNLOAD_DIR}/Godot_v${GODOT_VERSION}-${GODOT_SUB}_export_templates.tpz" -o /tmp/godot_templates.tpz
    mkdir -p "$TEMPLATES_DIR"
    unzip -o /tmp/godot_templates.tpz -d /tmp/godot_tpl
    mv /tmp/godot_tpl/templates/* "$TEMPLATES_DIR/"
    rm -rf /tmp/godot_templates.tpz /tmp/godot_tpl
fi
echo "==> Export templates installed."

# --- Step 3: Build glsl2wgsl tool (GLSL→WGSL converter for Bevy export) ---
echo "==> Building glsl2wgsl tool..."
if command -v cargo &> /dev/null; then
    (cd tools/glsl2wgsl && cargo build --release)
    GLSL2WGSL_BUILT=1
else
    echo "WARNING: Rust/cargo not found. glsl2wgsl tool will not be built."
    echo "         The 'wgpu (Bevy)' export will fall back to raw GLSL output."
    GLSL2WGSL_BUILT=0
fi

# --- Step 4: Setup environment ---
echo "==> Preparing project..."
cp -f material_maker/theme/default_theme_icons.svg material_maker/theme/default_theme_icons_export.svg

# --- Step 4: Export (run twice, like CI does) ---
mkdir -p ./build/mac
echo "==> Exporting Mac build (pass 1)..."
"$GODOT_BIN" --headless -v --export-release "Mac OSX" ./build/mac/${EXPORT_NAME}.zip
echo "==> Exporting Mac build (pass 2)..."
"$GODOT_BIN" --headless -v --export-release "Mac OSX" ./build/mac/${EXPORT_NAME}.zip

# --- Step 5: Unpack and make executable ---
echo "==> Unpacking app bundle..."
unzip -ao ./build/mac/${EXPORT_NAME}.zip -d ./build/mac
chmod +x "./build/mac/Material Maker.app/Contents/MacOS/Material Maker"
rm ./build/mac/${EXPORT_NAME}.zip

# --- Step 6: Fix icon ---
echo "==> Fixing application icon..."
sips -s format icns "./build/mac/Material Maker.app/Contents/Resources/icon.icns" --out "./build/mac/Material Maker.app/Contents/Resources/icon.icns"

# --- Step 7: Copy data folders ---
echo "==> Copying data folders into app bundle..."
cp -R ./addons/material_maker/nodes "./build/mac/Material Maker.app/Contents/MacOS"
cp -R ./material_maker/environments "./build/mac/Material Maker.app/Contents/MacOS"
cp -R ./material_maker/examples "./build/mac/Material Maker.app/Contents/MacOS"
cp -R ./material_maker/library "./build/mac/Material Maker.app/Contents/MacOS"
cp -R ./material_maker/meshes "./build/mac/Material Maker.app/Contents/MacOS"
cp -R ./material_maker/misc/export "./build/mac/Material Maker.app/Contents/MacOS"

# --- Step 7b: Copy glsl2wgsl tool into app bundle ---
if [ "${GLSL2WGSL_BUILT:-0}" = "1" ]; then
    echo "==> Copying glsl2wgsl into app bundle..."
    cp ./tools/glsl2wgsl/target/release/glsl2wgsl "./build/mac/Material Maker.app/Contents/MacOS/"
fi

# --- Step 8: Ad-hoc code sign ---
echo "==> Ad-hoc signing..."
codesign -s - --force --deep "./build/mac/Material Maker.app"

# --- Step 9: Remove quarantine ---
xattr -dr com.apple.quarantine "./build/mac/Material Maker.app"

echo ""
echo "==> Build complete!"
echo "==> App is at: ./build/mac/Material Maker.app"
echo "==> Run with: open \"./build/mac/Material Maker.app\""
