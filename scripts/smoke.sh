#!/bin/sh
# Call every route of a deployed edge-book Function and print status and timings.
# Usage: scripts/smoke.sh <base-url>
set -eu
BASE=${1:?usage: smoke.sh <base-url>}
UA='Mozilla/5.0 (smoke test)'

row() { # name, curl args...
  name=$1; shift
  hdr=$(mktemp)
  code=$(curl -s -o /dev/null -D "$hdr" -w '%{http_code} %{time_total}' -A "$UA" -H 'accept-language: en' "$@")
  fn=$(awk 'tolower($1)=="x-fn-us:"{gsub("\r","",$2);print $2}' "$hdr")
  seq=$(awk 'tolower($1)=="x-instance-seq:"{gsub("\r","",$2);print $2}' "$hdr")
  rm -f "$hdr"
  printf '%-34s %s  in-fn %6s us  instance-seq %s  total %ss\n' "$name" "${code% *}" "${fn:--}" "${seq:--}" "${code#* }"
}

CODE=$(curl -s "$BASE/api/meta" | sed -n 's/.*"redirect_samples":\["\([A-Z0-9]*\)".*/\1/p')
TOKEN=$(curl -s -X POST "$BASE/api/quote" -d '{"selection_id":"nfl-kc-buf-ml-away"}' | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')

row "GET /"                       "$BASE/"
row "GET /api/health"             "$BASE/api/health"
row "GET /api/meta"               "$BASE/api/meta"
row "GET /go/$CODE"               "$BASE/go/$CODE?state=NJ"
row "GET /api/redirect (legacy)"  "$BASE/api/redirect?path=/sportsbook/leagues/nfl"
row "GET /api/board"              "$BASE/api/board?segment=vip&fav=Lions"
row "POST /api/quote"             -X POST "$BASE/api/quote" -d '{"selection_id":"nfl-kc-buf-ml-away"}'
row "POST /api/precheck"          -X POST "$BASE/api/precheck" -d "{\"quote_token\":\"$TOKEN\",\"stake\":40,\"bettor\":{}}"
row "GET /api/simulate?n=1000"    "$BASE/api/simulate?n=1000"
row "GET /src/risk.rs"            "$BASE/src/risk.rs"
