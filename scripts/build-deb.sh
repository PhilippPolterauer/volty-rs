#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
output=${1:-dist}
expected_tag=${2:-}
if [[ $(dpkg --print-architecture) != amd64 ]]; then
    echo "The Volty Debian package requires an amd64 build host." >&2
    exit 1
fi

target_dir=$(realpath -m "${CARGO_TARGET_DIR:-target}")
version=$(cargo metadata --locked --no-deps --format-version 1 |
    jq -er '.packages[] | select(.name == "volty-tray") | .version')
if [[ -n $expected_tag && $expected_tag != "v$version" ]]; then
    echo "Release tag '$expected_tag' must match package version 'v$version'." >&2
    exit 1
fi
deb_version=${version/-/\~}
cargo build --locked --release --package volty-tray --target-dir "$target_dir"

work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
root=$work/volty
binary=$root/usr/bin/volty
install -Dm755 "$target_dir/release/volty" "$binary"
install -Dm644 packaging/linux/volty.desktop "$root/usr/share/applications/volty.desktop"
install -Dm644 crates/volty-tray/assets/volty-favicon.png \
    "$root/usr/share/icons/hicolor/192x192/apps/volty.png"
install -Dm644 LICENSE "$root/usr/share/doc/volty/copyright"

depends='systemd, dbus'
shopt -s nullglob
rules=(packaging/linux/*.rules)
if (( ${#rules[@]} )); then
    mkdir -p "$root/usr/lib/udev/rules.d"
    install -m644 -t "$root/usr/lib/udev/rules.d" "${rules[@]}"
    depends+=', udev'
fi

mkdir -p "$work/debian" "$root/DEBIAN"
cat > "$work/debian/control" <<'EOF'
Source: volty
Section: utils
Priority: optional
Maintainer: Volty maintainers <PhilippPolterauer@users.noreply.github.com>

Package: volty
Architecture: amd64
Description: Device battery monitor
EOF
shlibs=$(cd "$work" && dpkg-shlibdeps -O -e"$binary")
shlibs=${shlibs#shlibs:Depends=}
if [[ -n $shlibs ]]; then
    depends+=", $shlibs"
fi

cat > "$root/DEBIAN/control" <<EOF
Package: volty
Version: $deb_version
Architecture: amd64
Section: utils
Priority: optional
Maintainer: Volty maintainers <PhilippPolterauer@users.noreply.github.com>
Depends: $depends
Recommends: upower, bluez, xdg-utils
Homepage: https://github.com/PhilippPolterauer/volty-rs
Description: Device battery monitor
 Volty displays supported device battery levels in the system tray.
EOF

mkdir -p "$output"
asset="Volty-v$version-linux-amd64.deb"
dpkg-deb --root-owner-group --build "$root" "$output/$asset"
(cd "$output" && sha256sum "$asset" > "$asset.sha256")
