#!/bin/sh
# Clio's installer: puts `clio` and `clio-vault` of a release in ~/.local/bin, verified against
# that release's SHA256SUMS, and starts `clio setup` on the terminal.
#
#   curl -fsSL https://github.com/i-montes/clio/releases/latest/download/install.sh | sh
#
# Environment:
#   CLIO_VERSION=x.y.z      that release instead of the latest
#   CLIO_RELEASE_BASE       an https:// URL or a local folder with one release's files (VERSION,
#                           SHA256SUMS, clio-<target>, clio-vault-<target>) in place of GitHub
#   CLIO_ALLOW_ROOT=1       install for root (otherwise it asks to be run as your own user)
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

    if [ "$(id -u)" = 0 ] && [ "${CLIO_ALLOW_ROOT:-}" != 1 ]; then
        fail "ejecútalo como tu usuario, sin sudo" "run it as your own user, without sudo"
    fi
    case "$(uname -s)" in
        Linux) os=unknown-linux-gnu ;;
        Darwin) fail "macOS todavía no es compatible" "macOS is not supported yet" ;;
        *) fail "Clio no funciona en $(uname -s)" "Clio does not run on $(uname -s)" ;;
    esac
    case "$(uname -m)" in
        x86_64 | amd64) cpu=x86_64 ;;
        aarch64 | arm64) cpu=aarch64 ;;
        *) fail "Clio no funciona en un procesador $(uname -m)" "Clio does not run on a $(uname -m) processor" ;;
    esac
    target="$cpu-$os"

    command -v curl >/dev/null 2>&1 || fail "hace falta curl" "curl is needed"
    # Downloads $1 to $2: https only, and a redirect may not leave it; a local folder is read with
    # curl's file://.
    get() {
        case "$1" in
            https://*) curl -fsL --retry 3 --proto '=https' --proto-redir '=https' --tlsv1.2 -o "$2" "$1" ;;
            *) curl -fsL --proto '=file' -o "$2" "$1" ;;
        esac
    }
    # The sum is the first word of the tool's line: split on purpose.
    # shellcheck disable=SC2046
    if command -v sha256sum >/dev/null 2>&1; then
        sum() { set -- $(sha256sum "$1") && printf '%s' "$1"; }
    elif command -v shasum >/dev/null 2>&1; then
        sum() { set -- $(shasum -a 256 "$1") && printf '%s' "$1"; }
    else
        fail "hace falta sha256sum o shasum" "sha256sum or shasum is needed"
    fi

    bin="${HOME:?}/.local/bin"
    for name in clio clio-vault; do
        [ -d "$bin/$name" ] && fail "$bin/$name es una carpeta: quítala" "$bin/$name is a folder: remove it"
    done
    mkdir -p "$bin" || fail "no pude crear $bin" "could not create $bin"
    # Beside the destination (same filesystem): each binary is put in place with one rename.
    tmp=$(mktemp -d "$bin/.clio-install.XXXXXX") || fail "no pude escribir en $bin" "could not write to $bin"
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 1' INT TERM HUP

    base="${CLIO_RELEASE_BASE:-}"
    version=''
    if [ -n "$base" ]; then
        case "$base" in
            https://* | file://*) ;;
            http://*) fail "CLIO_RELEASE_BASE tiene que ser https:// o una carpeta" "CLIO_RELEASE_BASE must be https:// or a folder" ;;
            /*) base="file://$base" ;;
            *) base="file://$PWD/$base" ;;
        esac
        base="${base%/}"
        printf 'clio: instalando desde %s · installing from %s\n' "$base" "$base"
        get "$base/VERSION" "$tmp/VERSION" || fail "no pude descargar $base/VERSION" "could not download $base/VERSION"
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
        fail "no pude descargar Clio $version: no se instaló nada" "could not download Clio $version: nothing was installed"
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
            fail "no pude descargar $asset: no se instaló nada" "could not download $asset: nothing was installed"
        [ "$(sum "$tmp/$name")" = "$want" ] ||
            fail "$asset no coincide con su suma de verificación: no se instaló nada" "$asset does not match its checksum: nothing was installed"
        chmod 755 "$tmp/$name" || fail "no pude preparar $asset" "could not prepare $asset"
    done
    trap '' INT TERM HUP
    # shellcheck disable=SC2015 # either rename failing is the same failure
    mv -f "$tmp/clio-vault" "$bin/clio-vault" && mv -f "$tmp/clio" "$bin/clio" ||
        fail "no pude poner Clio en $bin" "could not put Clio in $bin"
    trap - INT TERM HUP
    rm -rf "$tmp"
    trap - EXIT

    # ~/.local/bin on the PATH from the next terminal on: one line in the login profile (and in
    # zsh's), added once.
    mark='# Clio: ~/.local/bin on the PATH'
    add_path() {
        if [ -f "$1" ]; then
            while IFS= read -r l || [ -n "$l" ]; do
                [ "$l" = "$mark" ] && return 0
            done <"$1"
        fi
        # shellcheck disable=SC2016 # the line goes in as written, to expand at login
        printf '\n%s\n%s\n' "$mark" 'export PATH="$HOME/.local/bin:$PATH"' >>"$1"
    }
    case ":${PATH:-}:" in
        *":$bin:"*) ;;
        *)
            if [ -f "$HOME/.bash_profile" ]; then add_path "$HOME/.bash_profile"; else add_path "$HOME/.profile"; fi
            case "${SHELL:-}" in */zsh) add_path "$HOME/.zshrc" ;; esac
            ;;
    esac
    # `curl | sh` gives the script, not the person, as standard input: the setup reads the terminal.
    if (: </dev/tty) 2>/dev/null; then
        # --fresh: what was just installed is not looked for again.
        exec "$bin/clio" setup --fresh </dev/tty
    fi
    printf '%s setup\n' "$bin/clio"
}

clio_install "$@"
