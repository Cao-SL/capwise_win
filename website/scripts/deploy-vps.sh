#!/usr/bin/env bash
set -euo pipefail

host="${CAPWISE_HOST:-qiaomu-la}"
site=/var/www/capwise
headers=$(mktemp)
trap 'rm -- "$headers"' EXIT
cd "$(dirname "$0")/.."

npm run build --silent
version=$(grep -o 'styles.css?v=[0-9a-f]*' dist/index.html | head -1 | cut -d= -f2)
find dist -type f \( -name '*.html' -o -name '*.css' -o -name '*.js' -o -name '*.txt' -o -name '*.xml' -o -name '*.json' -o -name '*.webmanifest' -o -name '*.svg' \) \
	-exec gzip -9 -k -f {} \;

{
	awk '/^\/\*$/{on=1; next} /^\//{on=0} on && NF {sub(/^ +/, ""); split($0, kv, ": "); printf "add_header %s \"%s\" always;\n", kv[1], substr($0, length(kv[1]) + 3)}' dist/_headers
} > "$headers"

rsync -az --delete --exclude _headers dist/ "$host:$site/releases/$version/"
scp -q deploy/nginx-capwise.conf "$host:/etc/nginx/sites-available/capwise"
scp -q "$headers" "$host:/etc/nginx/snippets/capwise-headers.conf"
ssh "$host" "set -e
	ln -sfn /etc/nginx/sites-available/capwise /etc/nginx/sites-enabled/capwise
	nginx -t -q
	ln -sfn $site/releases/$version $site/current.next && mv -T $site/current.next $site/current
	systemctl reload nginx"
echo "Deployed capwise $version to $host"
