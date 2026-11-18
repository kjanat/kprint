#!/bin/sh
set -eu

tools="${HOME}/.local"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$tools/bin"
export PATH="$tools/bin:$PATH"
cd "$work"

unpack() {
	archive=${1##*/}
	fetch -T 60 -o "$archive" "$1"
	if [ "$(sha256 -q "$archive")" != "$2" ]; then
		printf 'Checksum mismatch for %s\n' "$archive" >&2
		exit 1
	fi
	tar -xf "$archive"
	cd "${archive%%.tar.*}"
}

# FreeBSD base provides clang, the linker, and the OpenSSL development files.
(
	# renovate: devel/gmake
	unpack https://ftp.gnu.org/gnu/make/make-4.4.1.tar.lz \
		8814ba072182b605d156d7589c19a43b89fc58ea479b9355146160946f8cf6e9
	./configure --disable-nls
	sh build.sh
	install -m 755 make "$tools/bin/gmake"
)

(
	# renovate: devel/pkgconf
	unpack https://distfiles.dereferenced.org/pkgconf/pkgconf-3.0.9.tar.xz \
		6ab2d3b8087dcf500e34003a6a8934dca55d0a3d6066aeafb03c352db3e68bfd
	./configure --prefix="$tools" --disable-shared
	gmake -j 2
	gmake install
	ln -sf pkgconf "$tools/bin/pkg-config"
)

(
	# renovate: ftp/curl
	unpack https://curl.se/download/curl-8.23.0.tar.xz \
		8dadd7d2536a314f982ad05a651b713f426bf7d02cc2c76cffb9796f2218e6c5
	./configure --prefix="$tools" --disable-shared --with-openssl --with-ca-fallback \
		--without-libpsl --disable-ldap --disable-ldaps
	gmake -j 2
	gmake install
)

(
	# renovate: devel/git
	unpack https://www.kernel.org/pub/software/scm/git/git-2.56.1.tar.xz \
		f0b0bf89264b9bd2e1c640363e7529643989da76281600473c50745cd34a607f
	gmake -j 2 PROFILE= prefix="$tools" CURL_CONFIG="$tools/bin/curl-config" \
		NO_EXPAT=YesPlease NO_GETTEXT=YesPlease NO_ICONV=YesPlease \
		NO_PERL=YesPlease NO_PYTHON=YesPlease NO_RUST=YesPlease NO_TCLTK=YesPlease install
)

curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o rustup-init.sh
sh rustup-init.sh -y --profile minimal --default-toolchain none
