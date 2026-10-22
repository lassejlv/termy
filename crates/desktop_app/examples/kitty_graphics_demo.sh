#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
image="${1:-${script_dir}/assets/kitty-demo.png}"

if [[ ! -f "${image}" ]]; then
  printf 'Image not found: %s\n' "${image}" >&2
  exit 1
fi

# Kitty graphics protocol: transmit and display a PNG 40 terminal cells wide.
# Rows follow the image's aspect ratio; giving both c and r would stretch it.
# Stream 4096-byte Base64 chunks so larger images do not live in one shell variable.
first_chunk=1

emit_chunk() {
  local more="$1"
  local chunk="$2"
  if ((first_chunk)); then
    # Leave image and placement IDs anonymous so each invocation creates a
    # distinct placement instead of replacing the previous demo image.
    printf '\033_Ga=T,f=100,t=d,c=40,C=0,q=2,m=%d;%s\033\\' \
      "${more}" "${chunk}"
    first_chunk=0
  else
    printf '\033_Gm=%d;%s\033\\' "${more}" "${chunk}"
  fi
}

exec 3< <(
  {
    base64 <"${image}" | tr -d '\r\n'
    # `fold` does not terminate its final partial line, but Bash `read` requires
    # that newline to report success instead of silently dropping the data.
    printf '\n'
  } | fold -w 4096
)
if ! IFS= read -r chunk <&3; then
  printf 'Image is empty: %s\n' "${image}" >&2
  exit 1
fi

while IFS= read -r next_chunk <&3; do
  emit_chunk 1 "${chunk}"
  chunk="${next_chunk}"
done
emit_chunk 0 "${chunk}"
exec 3<&-

printf '\r\n'
