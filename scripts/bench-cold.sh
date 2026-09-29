#!/bin/sh
# Measure cold-traffic behavior of a deployed edge-book Function.
#
# Usage: scripts/bench-cold.sh <base-url> [requests] [out.csv]
#
# Run it immediately after `spin aka deploy` to capture the first request a new
# deployment ever serves, then compare it with the rest of the run.
#
# Per request it records curl's DNS, connect, TLS and time-to-first-byte, plus
# two response headers set by the component:
#   x-fn-us         microseconds inside the handler
#   x-instance-seq  requests served by this instance (1 = brand-new instance)
#
# Each request opens a new connection (no keep-alive) so every sample pays the
# full network setup; that is the worst case a new visitor sees.
set -eu

BASE=${1:?usage: bench-cold.sh <base-url> [requests] [out.csv]}
N=${2:-100}
OUT=${3:-bench-results/cold-$(date -u +%Y%m%dT%H%M%SZ).csv}
mkdir -p "$(dirname "$OUT")"

echo "i,dns_ms,connect_ms,tls_ms,ttfb_ms,total_ms,fn_us,instance_seq,http" > "$OUT"
i=1
while [ "$i" -le "$N" ]; do
  hdr=$(mktemp)
  line=$(curl -s -o /dev/null -D "$hdr" \
    -w '%{time_namelookup},%{time_connect},%{time_appconnect},%{time_starttransfer},%{time_total},%{http_code}' \
    "$BASE/api/health?bench=$i")
  fn=$(awk 'tolower($1)=="x-fn-us:"{gsub("\r","",$2);print $2}' "$hdr")
  seq=$(awk 'tolower($1)=="x-instance-seq:"{gsub("\r","",$2);print $2}' "$hdr")
  rm -f "$hdr"
  echo "$line" | awk -F, -v i="$i" -v fn="$fn" -v seq="$seq" \
    '{printf "%d,%.1f,%.1f,%.1f,%.1f,%.1f,%s,%s,%s\n", i, $1*1000, $2*1000, $3*1000, $4*1000, $5*1000, fn, seq, $6}' >> "$OUT"
  i=$((i + 1))
done

# Summary. "Server share" = TTFB minus TLS handshake completion: the time from
# request sent to first byte back, which is where any start-up cost would show.
awk -F, 'NR==1{next}
  { n++; ttfb[n]=$5; srv[n]=$5-$4; fn[n]=$7; if ($8==1) fresh++ }
  END {
    asort(ttfb, t); asort(srv, s); asort(fn, f)
    printf "requests:                 %d\n", n
    printf "brand-new instance:       %d of %d (%.0f%%)\n", fresh, n, 100*fresh/n
    printf "first request TTFB:       %.1f ms\n", first_ttfb
    printf "TTFB p50 / p95:           %.1f / %.1f ms\n", t[int(n*0.5)+1], t[int(n*0.95)]
    printf "server share p50 / p95:   %.1f / %.1f ms  (TTFB minus TLS done)\n", s[int(n*0.5)+1], s[int(n*0.95)]
    printf "inside Function p50/p95:  %d / %d us\n", f[int(n*0.5)+1], f[int(n*0.95)]
  }
  NR==2 { first_ttfb=$5 }' "$OUT"
echo "raw: $OUT"
