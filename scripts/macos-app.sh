#!/bin/bash
# Crea AltKeeper.app attorno al binario `altkeeper` per macOS.
#
#   scripts/macos-app.sh <binario> <versione> <cartella di uscita>
#   per esempio: scripts/macos-app.sh target/release/altkeeper 0.3.0 dist
#
# Aperta con un doppio clic, l'app esegue `altkeeper tray`: icona nella barra dei menu (niente
# Dock, come AltServer), AltServer e pagina web; i dati stanno in
# ~/Library/Application Support/AltKeeper. La firma è ad-hoc (nessun account Apple Developer):
# basta per Apple Silicon, ma Gatekeeper chiede conferma al primo avvio (vedi README).
set -euo pipefail

bin=${1:?binario}
version=${2:?versione}
out=${3:?cartella di uscita}
root=$(cd "$(dirname "$0")/.." && pwd)

app="$out/AltKeeper.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/AltKeeper"
chmod +x "$app/Contents/MacOS/AltKeeper"

# Icona .icns dal logo (256x256).
iconset=$(mktemp -d)/AltKeeper.iconset
mkdir -p "$iconset"
for spec in 16:icon_16x16 32:icon_16x16@2x 32:icon_32x32 64:icon_32x32@2x \
            128:icon_128x128 256:icon_128x128@2x 256:icon_256x256; do
  size=${spec%%:*}
  sips -z "$size" "$size" "$root/assets/logo.png" --out "$iconset/${spec#*:}.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AltKeeper.icns"
rm -rf "$(dirname "$iconset")"

# LSUIElement: solo l'icona nella barra dei menu. NSLocalNetworkUsageDescription e
# NSBonjourServices: macOS chiede il permesso "Rete locale" per trovare l'iPhone e farsi trovare
# da AltStore.
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>AltKeeper</string>
  <key>CFBundleDisplayName</key><string>AltKeeper</string>
  <key>CFBundleIdentifier</key><string>io.github.altkeeper</string>
  <key>CFBundleExecutable</key><string>AltKeeper</string>
  <key>CFBundleIconFile</key><string>AltKeeper</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSLocalNetworkUsageDescription</key>
  <string>AltKeeper finds your iPhone on the local network and lets AltStore find it.</string>
  <key>NSBonjourServices</key>
  <array>
    <string>_altserver._tcp</string>
    <string>_remotepairing._tcp</string>
  </array>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null

codesign --force -s - "$app"
echo "$app"
