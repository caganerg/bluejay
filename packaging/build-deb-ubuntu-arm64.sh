#!/bin/sh
#
# Build bluejay_<version>_arm64.deb for Ubuntu 26.04 (resolute) on an amd64
# host.
#
# packaging/build-deb.sh builds for the host's own Debian release: it fetches
# that release's libc6 and libc6-dev and links the binary against them. An arm64
# package cannot be built that way on an amd64 machine, so this script does the
# same thing across architectures — Ubuntu's arm64 libc6 and libc6-dev come from
# ports.ubuntu.com, clang and LLD are pointed at them as a sysroot, and the
# binary is built for aarch64-unknown-linux-gnu. Using the release's own libc is
# what keeps the result runnable there: the symbols the link binds are the ones
# that release's loader defines, and the package's libc6 dependency is derived
# from them rather than guessed.
#
#     packaging/build-deb-ubuntu-arm64.sh
#
# Host requirements: clang, lld, dpkg-deb, curl, xz, and an aarch64 Rust
# standard library (`rustup target add aarch64-unknown-linux-gnu`).
#
# Environment:
#   BLUEJAY_DEB_SUITE    Ubuntu release to build for (default resolute)
#   BLUEJAY_DEB_MIRROR   archive to fetch arm64 packages from
#                        (default https://ports.ubuntu.com/ubuntu-ports)
#   BLUEJAY_DEB_CACHE    where the sysroot, debs and qemu are kept
#                        (default ~/.cache/bluejay-deb)
#   BLUEJAY_DEB_OUT      where the .deb is written (default target/deb)
#   BLUEJAY_DEB_GCC      GCC series whose libgcc/crt objects are used
#                        (default 15, the one resolute ships)
#   BLUEJAY_CLANG        clang to use (default: clang-22, clang-21, …, clang)
#   BLUEJAY_QEMU         qemu-aarch64 to run the load probe with (default:
#                        found on PATH, or fetched with `apt-get download
#                        qemu-user`)
#
set -eu

cd "$(dirname "$0")/.."

suite=${BLUEJAY_DEB_SUITE:-resolute}
mirror=${BLUEJAY_DEB_MIRROR:-https://ports.ubuntu.com/ubuntu-ports}
arch=arm64
multiarch=aarch64-linux-gnu
target=aarch64-unknown-linux-gnu

cache=${BLUEJAY_DEB_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/bluejay-deb}
out=${BLUEJAY_DEB_OUT:-target/deb}

sysroot=$cache/ubuntu-$suite-$arch
libdir=$sysroot/usr/lib/$multiarch

# --- host tools --------------------------------------------------------------

find_tool() {
    for name in "$@"; do
        if command -v "$name" >/dev/null 2>&1; then
            echo "$name"
            return
        fi
    done
    return 1
}

clang=${BLUEJAY_CLANG:-$(find_tool clang-22 clang-21 clang-20 clang || true)}
[ -n "$clang" ] || {
    echo "clang is not installed (try: apt install clang lld)" >&2
    exit 1
}

# clang's -fuse-ld takes the suffix of ld.lld, not a path.
lld=$(find_tool ld.lld ld.lld-22 ld.lld-21 ld.lld-20 || true)
[ -n "$lld" ] || {
    echo "LLD is not installed (try: apt install lld)" >&2
    exit 1
}
lld=${lld#ld.}

cargo=${CARGO:-cargo}
rustc=${RUSTC:-rustc}

# The aarch64 std has to be installed for the toolchain that will do the build;
# without it cargo fails much later with a less obvious message.
if ! "$rustc" --print target-libdir --target "$target" >/dev/null 2>&1 ||
   [ ! -d "$("$rustc" --print target-libdir --target "$target")" ]; then
    echo "$rustc has no standard library for $target (try: rustup target add $target)" >&2
    exit 1
fi

# --- the release's glibc -----------------------------------------------------

index=$cache/Packages-$suite-$arch.xz

if [ ! -e "$sysroot/.ready" ]; then
    echo "preparing an Ubuntu $suite $arch sysroot in $sysroot" >&2
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

    # What the link needs: the release's C library and start files, and the
    # target's libgcc/crtbegin/crtend. libgcc-s1 carries the libgcc_s.so.1 that
    # the binary asks for at run time.
    fetch libc6
    fetch libc6-dev
    fetch "libgcc-${BLUEJAY_DEB_GCC:-15}-dev"
    fetch libgcc-s1
    fetch linux-libc-dev

    gccdir=$(ls -d "$sysroot/usr/lib/gcc/$multiarch"/*/ 2>/dev/null | sort -V | tail -1)
    [ -n "$gccdir" ] || { echo "no GCC directory in $sysroot" >&2; exit 1; }

    # glibc's linker scripts name what they stand for by absolute paths, which
    # the sysroot does not carry: the packages are extracted under /usr and
    # merged-usr's symlinks are not in them. Naming the members relative keeps
    # the whole search inside the sysroot.
    cat > "$libdir/libm.so" <<'EOF'
/* GNU ld script */
OUTPUT_FORMAT(elf64-littleaarch64)
GROUP ( libm.so.6 AS_NEEDED ( libmvec.so.1 ) )
EOF
    cat > "$libdir/libc.so" <<'EOF'
/* GNU ld script */
OUTPUT_FORMAT(elf64-littleaarch64)
GROUP ( libc.so.6 libc_nonshared.a AS_NEEDED ( ld-linux-aarch64.so.1 ) )
EOF

    # The probe below runs the binary under qemu, whose -L prefixes absolute
    # paths with the sysroot. These two links are what a merged-usr release has
    # and a bare extraction of the packages does not: /lib -> /usr/lib, and the
    # loader where the binary's interpreter line names it.
    ln -sfn usr/lib "$sysroot/lib"
    ln -sfn "$multiarch/ld-linux-aarch64.so.1" "$sysroot/usr/lib/ld-linux-aarch64.so.1"

    : > "$sysroot/.ready"
fi

gccdir=$(ls -d "$sysroot/usr/lib/gcc/$multiarch"/*/ 2>/dev/null | sort -V | tail -1)

# --- build -------------------------------------------------------------------

# The link needs the release's libc, crt objects and libgcc, and it invokes LLD
# rather than a target GCC (there is none on an amd64 host). --sysroot keeps
# every absolute path in the objects and scripts inside the extracted release.
link_args="-C link-arg=--target=$multiarch
 -C link-arg=--sysroot=$sysroot
 -C link-arg=--gcc-install-dir=$gccdir
 -C link-arg=-fuse-ld=$lld
 -C link-arg=-L$libdir"

echo "building $target against Ubuntu $suite's glibc in $libdir" >&2
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=$clang \
CC_aarch64_unknown_linux_gnu=$clang \
CFLAGS_aarch64_unknown_linux_gnu="--target=$multiarch --sysroot=$sysroot --gcc-install-dir=$gccdir -fuse-ld=$lld -L$libdir" \
PKG_CONFIG_ALLOW_CROSS=1 \
RUSTFLAGS="${RUSTFLAGS:-} $(printf '%s' "$link_args" | tr '\n' ' ')" \
    "$cargo" build --release --locked --target "$target"

bin=target/$target/release/Bluejay

# --- hold the build to its floor ---------------------------------------------

# readelf rather than objdump: the host's objdump need not know aarch64, and
# this reads the version definitions out of .gnu.version_r whatever the host.
libc_floor=$(readelf --version-info "$bin" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1 | sed 's/GLIBC_//')
gcc_floor=$(readelf --version-info "$bin" | grep -o 'GCC_[0-9.]*' | sort -uV | tail -1 | sed 's/GCC_//')
glibc=$(readelf --version-info "$libdir/libc.so.6" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1 | sed 's/GLIBC_//')

if [ -z "$libc_floor" ] || [ "$(printf '%s\n%s\n' "$glibc" "$libc_floor" | sort -V | tail -1)" != "$glibc" ]; then
    echo "the link bound symbols $suite does not have: $libc_floor > $glibc" >&2
    exit 1
fi

# Then prove it, rather than trust it: the binary is handed to the release's own
# dynamic linker under qemu with a Wayland display that is not there, so that it
# stops on the connection instead of opening a window. Reaching that message
# means the loader accepted the binary, every symbol version it asks for, and
# every library it opens by name.
qemu=${BLUEJAY_QEMU:-$(command -v qemu-aarch64 || true)}
if [ -z "$qemu" ] && command -v apt-get >/dev/null 2>&1; then
    qemu_dir=$cache/qemu-user
    if [ ! -x "$qemu_dir/usr/bin/qemu-aarch64" ]; then
        mkdir -p "$qemu_dir"
        if (cd "$qemu_dir" && apt-get download qemu-user >/dev/null 2>&1); then
            for deb in "$qemu_dir"/qemu-user_*.deb; do
                [ -f "$deb" ] && dpkg-deb -x "$deb" "$qemu_dir"
            done
        fi
    fi
    [ -x "$qemu_dir/usr/bin/qemu-aarch64" ] && qemu=$qemu_dir/usr/bin/qemu-aarch64
fi

if [ -n "$qemu" ]; then
    # The libraries the window's Wayland setup opens by name. They are not linked
    # into the binary, so the probe loads them out of the same sysroot; only the
    # connection attempt happens, so the renderer's EGL and Vulkan are not needed.
    if [ ! -e "$sysroot/.probe-ready" ]; then
        mkdir -p "$cache"
        [ -f "$index" ] || curl -fsSL -o "$index" \
            "$mirror/dists/$suite/main/binary-$arch/Packages.xz"
        for pkg in libwayland-client0 libwayland-cursor0 libwayland-egl1 \
                   libxkbcommon0 libffi8; do
            file=$(xz -dc "$index" | awk -v pkg="$pkg" '
                /^Package: / { want = ($2 == pkg) }
                want && /^Filename: / { print $2; exit }')
            [ -n "$file" ] || { echo "$pkg is not in $suite" >&2; exit 1; }
            deb=$cache/$(basename "$file")
            [ -f "$deb" ] || curl -fsSL -o "$deb" "$mirror/$file"
            dpkg-deb -x "$deb" "$sysroot"
        done
        : > "$sysroot/.probe-ready"
    fi

    probe=$(XDG_RUNTIME_DIR="$cache" WAYLAND_DISPLAY=bluejay-deb-packaging \
        timeout 120 "$qemu" -L "$sysroot" \
        -E LD_LIBRARY_PATH=/usr/lib/$multiarch \
        "$PWD/$bin" 2>&1 || true)
    case $probe in
        *"Could not find wayland compositor"*) ;;
        *)
            echo "Bluejay did not load under $suite's glibc:" >&2
            echo "$probe" >&2
            exit 1
            ;;
    esac
    echo "loads under $suite's glibc, which offers up to GLIBC_$glibc" >&2
else
    echo "qemu-aarch64 not found; skipped the load probe" >&2
    echo "the link is against $suite's glibc, which offers up to GLIBC_$glibc" >&2
fi

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
