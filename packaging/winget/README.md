# winget manifests

Templates for the `lassejlv.Termy` package in
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs). Placeholders:

| Placeholder | Example |
| --- | --- |
| `__VERSION__` | `1.2.3` (tag without the `v`) |
| `__TAG__` | `v1.2.3` (the release tag, which the asset names also use) |
| `__X64_SHA256__` | SHA-256 of `Termy-<tag>-windows-x64-Setup.exe`, from the release's `checksums.txt` |
| `__ARM64_SHA256__` | SHA-256 of `Termy-<tag>-windows-arm64-Setup.exe` |

## First submission

winget-releaser can only update a package that already exists, so the first
version is submitted by hand:

```sh
tag=v1.2.3
mkdir -p "manifests/l/lassejlv/Termy/${tag#v}"
for file in packaging/winget/*.yaml; do
  sed -e "s/__VERSION__/${tag#v}/g" -e "s/__TAG__/$tag/g" \
    -e "s/__X64_SHA256__/<x64 hash>/g" -e "s/__ARM64_SHA256__/<arm64 hash>/g" \
    "$file" > "manifests/l/lassejlv/Termy/${tag#v}/$(basename "$file")"
done
winget validate --manifest "manifests/l/lassejlv/Termy/${tag#v}"
winget install --manifest "manifests/l/lassejlv/Termy/${tag#v}"
```

Then open a pull request with that directory against `microsoft/winget-pkgs`
(or use `wingetcreate submit`).

## Later releases

The *Publish to winget* job in `.github/workflows/finalize-stable-release.yml`
opens the update pull request for each stable release when the `WINGET_TOKEN`
secret is set (see [docs/development.md](../../docs/development.md#winget)).
Keep these templates in sync when the installer's scopes, switches, or asset
names change.
