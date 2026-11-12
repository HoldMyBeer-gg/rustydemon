# rustydemon-bin (AUR)

Arch package that installs the prebuilt Linux binary from the GitHub
release, plus a desktop entry and icons.

Build and install from this directory:

    makepkg -si

The release workflow keeps this directory current: after a release is
published it runs `scripts/aur-bump.sh` and commits the new version and
checksums to main. To do the same by hand:

    scripts/aur-bump.sh 1.2.3 <sha256 of the Linux tarball>

Publishing to the AUR is still a manual push of three files (`PKGBUILD`,
`.SRCINFO`, `rustydemon.desktop`) to the AUR repository.
