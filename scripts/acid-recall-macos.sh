#!/usr/bin/env bash
# Builds dist/acid-recall-macos/ACID-RECALL-<version>-macOS.dmg: a universal
# (Apple silicon and Intel) "ACID RECALL.app", signed with a Developer ID and the hardened
# runtime, notarized and stapled; then the same for the .dmg. The image also holds an
# Applications link for drag-install, the guides the Windows ZIP ships and the licenses.
#
#   scripts/acid-recall-macos.sh
#
# Environment:
#   SIGN_IDENTITY      the codesign identity; default: the keychain's one valid
#                      "Developer ID Application" identity.
#   NOTARY_PROFILE     the notarytool keychain profile (default acid-recall), made once with
#                      `xcrun notarytool store-credentials acid-recall --apple-id ID --team-id TEAM`.
#   NOTARIZE=0         signs without notarizing: fine on this Mac, blocked by Gatekeeper elsewhere.
#   ARCHS              "aarch64 x86_64" (default), or one of them.
#   SOURCE_DATE_EPOCH  the build time (default now), the bundle's build number (scripts/version.sh).
# The public version is orb-ui's Cargo version, like the Windows package (midi-windows.yml).
#   BUNDLE_ID          default io.github.smldms.acid-recall.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
[ "$(uname -s)" = Darwin ] || { echo "macOS only (codesign, notarytool, hdiutil)" >&2; exit 1; }

export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(date +%s)}
pkgid=$(cargo pkgid --manifest-path "$root/Cargo.toml" -p orb-ui)
version=${pkgid##*[#@]}   # 1.0.0
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "bad orb-ui version: $pkgid" >&2; exit 1; }
build=$("$root/scripts/version.sh")   # 2026.1122.1730
built="$("$root/scripts/version.sh" --time) UTC"
bundle_id=${BUNDLE_ID:-io.github.smldms.acid-recall}
archs=${ARCHS:-aarch64 x86_64}
profile=dist
notary_profile=${NOTARY_PROFILE:-acid-recall}
out=${DIST:-$root/dist}/acid-recall-macos
name="ACID RECALL"
exe=ACID-RECALL

identity=${SIGN_IDENTITY:-}
if [ -z "$identity" ]; then
    identities=$(security find-identity -v -p codesigning |
        sed -n 's/.*"\(Developer ID Application: .*\)"$/\1/p' | sort -u)
    [ "$(printf '%s\n' "$identities" | grep -c .)" = 1 ] || {
        echo "need exactly one Developer ID Application identity, or SIGN_IDENTITY; found:" >&2
        printf '%s\n' "${identities:-(none)}" >&2
        exit 1
    }
    identity=$identities
fi
if [ "${NOTARIZE:-1}" != 0 ]; then
    # Fail before a long build when the credentials are missing or refused (an HTTP 403 means
    # the team must accept Apple's updated developer agreement).
    if ! check=$(xcrun notarytool history --keychain-profile "$notary_profile" 2>&1); then
        echo "notarytool refused profile '$notary_profile':" >&2
        echo "$check" | head -3 >&2
        echo "make one with: xcrun notarytool store-credentials $notary_profile" \
            "--apple-id ID --team-id TEAM; or set NOTARIZE=0" >&2
        exit 1
    fi
fi

# 1. One binary per architecture (fat LTO, stripped: Cargo.toml's dist profile), then lipo.
# macOS 11.0 (Big Sur) is the oldest, like scripts/build.sh.
export MACOSX_DEPLOYMENT_TARGET=11.0
target_dir=${CARGO_TARGET_DIR:-$root/target}
slices=()
for arch in $archs; do
    target=$arch-apple-darwin
    rustup target add "$target" >/dev/null
    cargo build --manifest-path "$root/Cargo.toml" --profile "$profile" --locked \
        --target "$target" -p orb-ui
    slices+=("$target_dir/$target/$profile/OpenRB-338")
done
rm -rf "$out"
mkdir -p "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
lipo -create "${slices[@]}" -output "$work/$exe"

# 2. The bundle. The icon is the brand logo (512 px) at the iconset sizes up to 512.
app=$work/stage/$name.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$work/$exe" "$app/Contents/MacOS/$exe"
chmod 755 "$app/Contents/MacOS/$exe"
iconset=$work/AcidRecall.iconset
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$root/assets/brand/acid-recall.png" \
        --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    [ "$double" -gt 512 ] && continue
    sips -z "$double" "$double" "$root/assets/brand/acid-recall.png" \
        --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AcidRecall.icns"

# The oldest macOS the binary runs on: the highest minos of its Mach-O slices.
minos=$(otool -arch all -l "$app/Contents/MacOS/$exe" | awk '$1 == "minos" { print $2 }' |
    sort -V | tail -1)
[ -n "$minos" ] || { echo "no minos in the Mach-O headers" >&2; exit 1; }

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>$exe</string>
    <key>CFBundleIconFile</key>
    <string>AcidRecall</string>
    <key>CFBundleIdentifier</key>
    <string>$bundle_id</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>$name</string>
    <key>CFBundleDisplayName</key>
    <string>$name</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$build</string>
    <key>OpenRBBuildTime</key>
    <string>$built</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.music</string>
    <key>LSMinimumSystemVersion</key>
    <string>$minos</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>ACID RECALL, a fork of OpenRB-338. GPL-3.0-only.</string>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null

# 3. Developer ID signature with the hardened runtime and a secure timestamp, both required by
# notarization. No entitlement: audio output, CoreMIDI and file access need none.
codesign --force --options runtime --timestamp --sign "$identity" "$app"
codesign --verify --strict --verbose=2 "$app"

notarize() {
    local file=$1 result
    result=$work/notary-$(basename "$file").json
    xcrun notarytool submit "$file" --keychain-profile "$notary_profile" --wait \
        --output-format json > "$result"
    if [ "$(plutil -extract status raw -o - "$result")" != Accepted ]; then
        cat "$result" >&2
        xcrun notarytool log "$(plutil -extract id raw -o - "$result")" \
            --keychain-profile "$notary_profile" >&2 || true
        exit 1
    fi
}

# 4. Notarize and staple the app, so it opens offline once copied out of the image.
if [ "${NOTARIZE:-1}" != 0 ]; then
    ditto -c -k --keepParent "$app" "$work/app.zip"
    notarize "$work/app.zip"
    xcrun stapler staple "$app"
fi

# 5. The image: the app, an Applications link, the guides and the licenses.
stage=$work/stage
ln -s /Applications "$stage/Applications"
mkdir -p "$stage/Guides"
cp "$root/docs/acid-recall.md" "$stage/Guides/ACID-RECALL.md"
cp "$root/docs/studio.md" "$stage/Guides/STUDIO.md"
cp "$root/docs/midi-controller.md" "$stage/Guides/MIDI.md"
cp "$root/docs/midi-clock.md" "$stage/Guides/MIDI-CLOCK.md"
cp "$root/docs/pattern-generator.md" "$stage/Guides/RANDOM.md"
cp "$root/docs/arpeggiator.md" "$stage/Guides/ARP.md"
cp "$root/docs/graphite.md" "$stage/Guides/GRAPHITE.md"
cp "$root/docs/sidechain.md" "$stage/Guides/SIDECHAIN.md"
cp "$root/docs/sampler-drums.md" "$stage/Guides/SAMPLER-DRUMS.md"
cp "$root/CHANGELOG.md" "$stage/CHANGELOG.md"
if [ -f "$root/docs/releases/$version.md" ]; then
    cp "$root/docs/releases/$version.md" "$stage/RELEASE-NOTES.md"
fi
cp "$root/LICENSE" "$stage/LICENSE"
cp "$root/docs/LICENSE-SYMPHONIA.txt" "$stage/LICENSE-SYMPHONIA.txt"
cat > "$stage/README.txt" <<EOF
$name $version for macOS 11 (Big Sur) or newer, Apple silicon and Intel.

Install: drag "$name.app" onto Applications, then open it from there.

Audio: Options > Preferences / Audio chooses the CoreAudio output, its sample rate and
buffer size. MIDI: Options > MIDI Controller lists the CoreMIDI inputs (controllers, IAC
buses, network sessions); MIDI > MIDI CLOCK / SYNC sends or follows MIDI Clock.
The guides are in the Guides folder; https://acidrecall.com/

ACID RECALL is a fork of OpenRB-338 (https://github.com/pingw33n/OpenRB-338), licensed under
the GNU General Public License, version 3 only (GPL-3.0-only; see LICENSE).
EOF

dmg=$out/ACID-RECALL-$version-macOS.dmg
# hdiutil sometimes reports "Resource busy" right after the files were written; retry.
for attempt in 1 2 3; do
    hdiutil create -quiet -volname "$name" -srcfolder "$stage" -fs HFS+ -format UDZO \
        -ov "$dmg" && break
    [ "$attempt" -lt 3 ] || { echo "hdiutil create failed" >&2; exit 1; }
    sleep 5
done
codesign --force --timestamp --sign "$identity" "$dmg"

# 6. Notarize and staple the image, then check both as Gatekeeper would.
if [ "${NOTARIZE:-1}" != 0 ]; then
    notarize "$dmg"
    xcrun stapler staple "$dmg"
    spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
    spctl --assess --type execute --verbose=2 "$app"
fi
ls -l "$dmg"
echo "made $dmg (version $version, build $build, $(lipo -archs "$app/Contents/MacOS/$exe"), macOS $minos or newer)"
