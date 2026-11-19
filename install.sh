#!/bin/sh
# Clio's installer: puts `clio` and `clio-vault` of a release in ~/.local/bin, verified against
# that release's SHA256SUMS, and starts `clio setup` on the terminal.
#
#   curl -fsSL https://raw.githubusercontent.com/i-montes/clio/main/install.sh | sh
#
# Environment:
#   CLIO_VERSION=x.y.z      that release instead of the latest
#   CLIO_RELEASE_BASE       a URL or a local folder with one release's files (VERSION, SHA256SUMS,
#                           clio-<target>, clio-vault-<target>) in place of GitHub
#
# Everything is inside one function called on the last line: a download cut anywhere runs nothing.

clio_install() {
    set -u
    releases=https://github.com/i-montes/clio/releases

    # One line, in both languages, and nothing is installed.
    fail() {
        printf 'clio: %s · %s\n' "$1" "$2" >&2
        exit 1
    }

    case "$(uname -s)" in
        Linux) os=unknown-linux-gnu ;;
        Darwin) os=apple-darwin ;;
        *) fail "Clio no funciona en $(uname -s)" "Clio does not run on $(uname -s)" ;;
    esac
    case "$(uname -m)" in
        x86_64 | amd64) cpu=x86_64 ;;
        aarch64 | arm64) cpu=aarch64 ;;
        *) fail "Clio no funciona en un procesador $(uname -m)" "Clio does not run on a $(uname -m) processor" ;;
    esac
    target="$cpu-$os"

    # Downloads $1 to $2. Over https a redirect may not leave https; a local folder is read with
    # curl's file://.
    if command -v curl >/dev/null 2>&1; then
        get() {
            case "$1" in
                https://*) curl -fsL --retry 3 --proto =https --proto-redir =https -o "$2" "$1" ;;
                *) curl -fsL --retry 3 -o "$2" "$1" ;;
            esac
        }
    elif command -v wget >/dev/null 2>&1; then
        get() {
            case "$1" in
                https://*) wget -q --tries=3 --https-only -O "$2" "$1" ;;
                *) wget -q --tries=3 -O "$2" "$1" ;;
            esac
        }
    else
        fail "hace falta curl o wget" "curl or wget is needed"
    fi
    if command -v sha256sum >/dev/null 2>&1; then
        sum() { set -- $(sha256sum "$1") && printf '%s' "$1"; }
    elif command -v shasum >/dev/null 2>&1; then
        sum() { set -- $(shasum -a 256 "$1") && printf '%s' "$1"; }
    else
        fail "hace falta sha256sum o shasum" "sha256sum or shasum is needed"
    fi

    bin="${HOME:?}/.local/bin"
    mkdir -p "$bin" || fail "no pude crear $bin" "could not create $bin"
    # Beside the destination (same filesystem): each binary is put in place with one rename.
    tmp=$(mktemp -d "$bin/.clio-install.XXXXXX") || fail "no pude escribir en $bin" "could not write to $bin"
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 1' INT TERM HUP

    base="${CLIO_RELEASE_BASE:-}"
    version=''
    if [ -n "$base" ]; then
        case "$base" in
            http://* | https://* | file://*) ;;
            /*) base="file://$base" ;;
            *) base="file://$PWD/$base" ;;
        esac
        base="${base%/}"
        get "$base/VERSION" "$tmp/VERSION" || fail "no pude bajar $base/VERSION" "could not download $base/VERSION"
        read -r version <"$tmp/VERSION" || :
    elif [ -n "${CLIO_VERSION:-}" ]; then
        version="${CLIO_VERSION#v}"
        base="$releases/download/v$version"
    else
        get "$releases/latest/download/VERSION" "$tmp/VERSION" ||
            fail "no pude saber cuál es la última versión" "could not find the latest version"
        read -r version <"$tmp/VERSION" || :
        base="$releases/download/v$version"
    fi
    case "$version" in
        '' | *[!0-9.]* | .* | *. | *..*) fail "versión no válida: $version" "not a valid version: $version" ;;
    esac

    get "$base/SHA256SUMS" "$tmp/SHA256SUMS" ||
        fail "no pude bajar Clio $version: no se instaló nada" "could not download Clio $version: nothing was installed"
    # Both are downloaded and verified before either is put in place.
    for name in clio-vault clio; do
        asset="$name-$target"
        want=''
        while read -r s f || [ -n "${s:-}" ]; do
            [ "${f#\*}" = "$asset" ] && want="$s"
            s=''
        done <"$tmp/SHA256SUMS"
        [ -n "$want" ] || fail "$asset no está en SHA256SUMS: no se instaló nada" "$asset is not in SHA256SUMS: nothing was installed"
        get "$base/$asset" "$tmp/$name" ||
            fail "no pude bajar $asset: no se instaló nada" "could not download $asset: nothing was installed"
        [ "$(sum "$tmp/$name")" = "$want" ] ||
            fail "$asset no coincide con su suma: no se instaló nada" "$asset does not match its checksum: nothing was installed"
        chmod 755 "$tmp/$name" || fail "no pude preparar $asset" "could not prepare $asset"
    done
    trap '' INT TERM HUP
    mv -f "$tmp/clio-vault" "$bin/clio-vault" && mv -f "$tmp/clio" "$bin/clio" ||
        fail "no pude poner Clio en $bin" "could not put Clio in $bin"
    trap - INT TERM HUP
    rm -rf "$tmp"
    trap - EXIT

    case ":${PATH:-}:" in
        *":$bin:"*) ;;
        *) printf 'clio: añade %s a tu PATH · add %s to your PATH\n' "$bin" "$bin" ;;
    esac
    # `curl | sh` gives the script, not the person, as standard input: the setup reads the terminal.
    if (: </dev/tty) 2>/dev/null; then
        exec "$bin/clio" setup </dev/tty
    fi
    printf '%s setup\n' "$bin/clio"
}

clio_install "$@"
