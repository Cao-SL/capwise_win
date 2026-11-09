#!/usr/bin/env bash
set -euo pipefail

input="${1:?Usage: package-capwise-deb.sh <Cap.deb> <Capwise.deb>}"
output="${2:?Usage: package-capwise-deb.sh <Cap.deb> <Capwise.deb>}"
work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT
root="$work/root"

[[ "$(dpkg-deb -f "$input" Package)" == cap ]]
[[ "$(dpkg-deb -f "$input" Architecture)" == amd64 ]]
dpkg-deb --raw-extract "$input" "$root"

test -x "$root/usr/bin/Cap"
test -d "$root/usr/lib/cap"
test "$(cat "$root/usr/lib/cap/package-format")" = deb
mkdir -p "$root/opt/capwise/lib"
mv "$root/usr/bin" "$root/opt/capwise/bin"
mv "$root/usr/lib/cap" "$root/opt/capwise/lib/cap"
rmdir "$root/usr/lib" 2>/dev/null || true
mkdir -p "$root/usr/bin"
ln -s /opt/capwise/bin/Cap "$root/usr/bin/capwise"

applications="$root/usr/share/applications"
test -f "$applications/Cap.desktop"
mv "$applications/Cap.desktop" "$applications/Capwise.desktop"
sed -i -E 's/^Name=Cap$/Name=Capwise/; s|^Exec=(/usr/bin/)?Cap([[:space:]].*)?$|Exec=capwise\2|; s/^Icon=Cap$/Icon=Capwise/' "$applications/Capwise.desktop"
grep -qx 'Name=Capwise' "$applications/Capwise.desktop"
grep -Eq '^Exec=capwise($|[[:space:]])' "$applications/Capwise.desktop"
grep -qx 'Icon=Capwise' "$applications/Capwise.desktop"

while IFS= read -r -d '' icon; do
  mv "$icon" "$(dirname "$icon")/Capwise.png"
done < <(find "$root/usr/share/icons" -type f -name Cap.png -print0)
test -n "$(find "$root/usr/share/icons" -type f -name Capwise.png -print -quit)"

while IFS= read -r -d '' mime; do
  mv "$mime" "$(dirname "$mime")/Capwise.xml"
done < <(find "$root/usr/share" -type f -name Cap.xml -print0)

sed -i 's/^Package: cap$/Package: capwise/; s/^Description:.*/Description: Capwise screen recorder and editor/' "$root/DEBIAN/control"
(
  cd "$root"
  find . -type f ! -path './DEBIAN/*' -print0 | sort -z | xargs -0 md5sum |
    sed 's|  \./|  |' > DEBIAN/md5sums
)
dpkg-deb --build --root-owner-group -Zxz "$root" "$output"
[[ "$(dpkg-deb -f "$output" Package)" == capwise ]]
