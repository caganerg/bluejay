#!/bin/sh
#
# Build bluejay_<version>_<arch>.deb, a package for Debian 13 and newer.
#
# Bluejay links libc, libgcc_s and libm, and opens Wayland by name at runtime,
# so the one thing that decides which Debian releases the binary runs on is the
# glibc it was linked against. Building on a newer machine than the release
# being packaged for is not safe: glibc 2.43 re-versioned the C float maths, so
# a link against it binds `atan2f` and its neighbours to GLIBC_2.43, and Debian
# 13 — glibc 2.41 — refuses the binary at load time with
#
#     version `GLIBC_2.43' not found (required by Bluejay)
#
# So the release's own libc6 and libc6-dev are fetched from the archive, the
# link is pointed at them, and the result is loaded with that release's own
# dynamic linker before a package is made of it. Nothing else comes from the
# release: the compiler, the toolchain and the build itself stay the host's.
#
#     packaging/build-deb.sh
#
# Environment:
#   BLUEJAY_DEB_MIRROR  archive to fetch the release from (default deb.debian.org)
#   BLUEJAY_DEB_CACHE   where the release and the staging tree are kept
#                       (default ~/.cache/bluejay-deb)
#   BLUEJAY_DEB_OUT     where the .deb is written (default target/deb)
#
set -eu

cd "$(dirname "$0")/.."

# The oldest Debian release the package claims to support; its glibc is the
# floor the binary has to be linked against, and the one the package depends on.
suite=${BLUEJAY_DEB_SUITE:-trixie}

mirror=${BLUEJAY_DEB_MIRROR:-https://deb.debian.org/debian}
cache=${BLUEJAY_DEB_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/bluejay-deb}
out=${BLUEJAY_DEB_OUT:-target/deb}

arch=$(dpkg --print-architecture)
multiarch=$(dpkg-architecture -qDEB_HOST_MULTIARCH)

sysroot=$cache/$suite-$multiarch
libdir=$sysroot/usr/lib/$multiarch
# The host's libraries go after the release's on the probe's search path, for
# the one thing the sysroot does not carry: libgcc_s.
hostlib=/usr/lib/$multiarch

# --- the release's glibc -----------------------------------------------------

index=$cache/Packages-$suite-$arch.xz

if [ ! -e "$sysroot/.ready" ]; then
    echo "preparing a $suite sysroot in $sysroot" >&2
    rm -rf "$sysroot"
    mkdir -p "$cache" "$sysroot"

    [ -f "$index" ] || curl -fsSL -o "$index" \
        "$mirror/dists/$suite/main/binary-$arch/Packages.xz"

    fetch() {
        file=$(xz -dc "$index" | awk -v pkg="$1" '
            /^Package: / { want = ($2 == pkg) }
            want && /^Filename: / { print $2; exit }')
        [ -n "$file" ] || { echo "$1 is not in $suite" >&2; exit 1; }
        deb=$cache/$(basename "$file")
        [ -f "$deb" ] || curl -fsSL -o "$deb" "$mirror/$file"
        dpkg-deb -x "$deb" "$sysroot"
    }

    fetch libc6
    fetch libc6-dev

    # glibc's linker scripts name what they stand for by absolute /lib/... paths,
    # which the linker resolves on the *host* rather than in the sysroot — so the
    # newer machine's libm would be the one bound, and the check below would fail
    # exactly as described at the top. Naming them relative keeps the whole
    # search inside the sysroot.
    cat > "$libdir/libm.so" <<'EOF'
/* GNU ld script */
GROUP ( libm.so.6  AS_NEEDED ( libmvec.so.1 ) )
EOF
    cat > "$libdir/libc.so" <<'EOF'
/* GNU ld script */
GROUP ( libc.so.6 libc_nonshared.a  AS_NEEDED ( /lib64/ld-linux-x86-64.so.2 ) )
EOF

    : > "$sysroot/.ready"
fi

# --- build -------------------------------------------------------------------

echo "building against $suite's glibc in $libdir" >&2
RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-L$libdir" cargo build --release --locked
bin=target/release/Bluejay

# --- hold the build to its floor ---------------------------------------------

highest() { objdump -T "$bin" | grep -o "$1_[0-9.]*" | sort -uV | tail -1 | sed "s/$1_//"; }

libc_floor=$(highest GLIBC)
gcc_floor=$(highest GCC)
# The newest version the release's own libc defines: what the link may use, and
# what the package may promise.
glibc=$(objdump -T "$libdir/libc.so.6" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1 | sed 's/GLIBC_//')

if [ "$(printf '%s\n%s\n' "$glibc" "$libc_floor" | sort -V | tail -1)" != "$glibc" ]; then
    echo "the link bound symbols $suite does not have: $libc_floor > $glibc" >&2
    exit 1
fi

# Then prove it, rather than trust it: the binary is handed to the release's own
# dynamic linker with a Wayland display that is not there, so that it stops on
# the connection instead of opening a window. Reaching that message means the
# loader accepted the binary, every symbol version it asks for, and every
# library it opens by name.
loader=$(ls "$sysroot"/usr/lib/"$multiarch"/ld-linux-*.so.* 2>/dev/null | head -1)
probe=$(XDG_RUNTIME_DIR="$cache" WAYLAND_DISPLAY=bluejay-deb-packaging \
    timeout 60 "$loader" --library-path "$libdir:$hostlib" "$PWD/$bin" 2>&1 || true)
case $probe in
    *"Could not find wayland compositor"*) ;;
    *)
        echo "Bluejay did not load under $suite's glibc:" >&2
        echo "$probe" >&2
        exit 1
        ;;
esac

echo "loads under $suite's glibc, which offers up to GLIBC_$glibc" >&2

# --- package -----------------------------------------------------------------

version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
stage=$out/bluejay
deb=$out/bluejay_${version}_${arch}.deb
rm -rf "$stage" "$deb"
mkdir -p "$stage/DEBIAN"

install -Dm755 "$bin"                                "$stage/usr/bin/Bluejay"
install -Dm644 assets/Bluejay.desktop                "$stage/usr/share/applications/Bluejay.desktop"
install -Dm644 assets/logo.png                       "$stage/usr/share/icons/hicolor/512x512/apps/Bluejay.png"
install -Dm644 packaging/debian/copyright            "$stage/usr/share/doc/bluejay/copyright"
install -Dm644 assets/fonts/AdwaitaSans-OFL.txt      "$stage/usr/share/doc/bluejay/AdwaitaSans-OFL.txt"
install -Dm644 assets/fonts/JetBrainsMono-OFL.txt    "$stage/usr/share/doc/bluejay/JetBrainsMono-OFL.txt"
gzip -9n -c packaging/debian/changelog >               "$stage/usr/share/doc/bluejay/changelog.gz"

# Installed-Size counts what is unpacked, so it is asked for before the control
# area is added.
size=$(du -sk "$stage" | cut -f1)

cat > "$stage/DEBIAN/control" <<EOF
Package: bluejay
Version: $version
Architecture: $arch
Maintainer: caganerg <caganergun@tutanota.com>
Installed-Size: $size
Depends: libc6 (>= $libc_floor), libgcc-s1 (>= $gcc_floor), libegl1, libwayland-client0, libwayland-egl1, libxkbcommon0
Recommends: libgl1-mesa-dri | mesa-vulkan-drivers
Section: editors
Priority: optional
Homepage: https://github.com/caganerg/bluejay
Description: native markdown note app with a tree, an editor and a preview
 Bluejay is a small desktop markdown note editor: a file tree, a plain text
 editor and a live preview, side by side. It draws its own window out of
 eframe/egui rather than embedding a web engine, and carries its own typefaces,
 so it looks the same on every machine and takes nothing from the desktop's
 theme.
 .
 Notes are plain .md files in a folder you pick. It works entirely offline:
 nothing in it reaches for the network, a link in a note is drawn but never
 opened, and an image is shown as its link rather than fetched. The open note
 is written 600 ms after you stop typing, to a temporary file beside it and
 renamed over it, and if the note changed on disk in the meantime the save
 stops and asks which copy to keep.
 .
 Every window Bluejay shows is one it draws itself. The only shared libraries
 it links are libc, libgcc_s and libm; Wayland and the renderer are opened by
 name at runtime. It is Wayland-only: the X11 backend is compiled out, not
 merely disabled.
EOF

install -Dm755 packaging/debian/postinst "$stage/DEBIAN/postinst"

# Same bytes in, same bytes out, as far as a build of this tree can promise it.
if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
    SOURCE_DATE_EPOCH=$(git log -1 --format=%ct 2>/dev/null || date +%s)
    export SOURCE_DATE_EPOCH
fi

dpkg-deb --build --root-owner-group "$stage" "$deb"

echo "wrote $deb"
dpkg-deb --info "$deb"
