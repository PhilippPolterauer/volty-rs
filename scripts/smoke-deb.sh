#!/usr/bin/env bash
set -euo pipefail

asset=$(realpath "$1")
if [[ $(dpkg-query -W -f='${db:Status-Status}' volty 2>/dev/null || true) == installed ]]; then
    echo "Run this smoke test on a system where Volty is not installed." >&2
    exit 1
fi
(cd "$(dirname "$asset")" && sha256sum --check "$(basename "$asset").sha256")
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
chmod 755 "$work"
test "$(dpkg-deb -f "$asset" Architecture)" = amd64
version=$(dpkg-deb -f "$asset" Version)

# Use the same payload at an older version to exercise dpkg's upgrade path.
dpkg --compare-versions 0~smoke lt "$version"
dpkg-deb -R "$asset" "$work/previous"
sed -i 's/^Version:.*/Version: 0~smoke/' "$work/previous/DEBIAN/control"
dpkg-deb --root-owner-group --build "$work/previous" "$work/previous.deb"

export XDG_STATE_HOME="$work/state"
mkdir -p "$XDG_STATE_HOME/volty"
printf 'existing Volty state\n' > "$work/expected"
cp "$work/expected" "$XDG_STATE_HOME/volty/preserved"
launcher=/usr/share/applications/volty.desktop
icon=/usr/share/icons/hicolor/192x192/apps/volty.png
shopt -s nullglob
rules=("$work/previous"/usr/lib/udev/rules.d/*.rules)

check_install() {
    test "$(dpkg-query -W -f='${Version}' volty)" = "$1"
    /usr/bin/volty --help | grep -w tray
    busctl --version
    desktop-file-validate "$launcher"
    grep -Fx 'Exec=volty tray' "$launcher"
    grep -Fx 'Icon=volty' "$launcher"
    test -s "$icon"
    for rule in "${rules[@]}"; do
        cmp "$rule" "/usr/lib/udev/rules.d/${rule##*/}"
    done
    cmp "$work/expected" "$XDG_STATE_HOME/volty/preserved"
}

sudo apt-get install -y --no-install-recommends "$work/previous.deb"
check_install '0~smoke'
sudo apt-get install -y --no-install-recommends "$asset"
check_install "$version"
sudo apt-get remove -y volty
test ! -e /usr/bin/volty
test ! -e "$launcher"
test ! -e "$icon"
for rule in "${rules[@]}"; do
    test ! -e "/usr/lib/udev/rules.d/${rule##*/}"
done
cmp "$work/expected" "$XDG_STATE_HOME/volty/preserved"
