# rustydemon-bin (AUR)

Arch package that installs the prebuilt Linux binary from the GitHub
release, plus a desktop entry and icons.

Build and install from this directory:

    makepkg -si

For a new release: set `pkgver`, refresh the checksums with
`updpkgsums`, regenerate `.SRCINFO` with
`makepkg --printsrcinfo > .SRCINFO`, and push the three files
(`PKGBUILD`, `.SRCINFO`, `rustydemon.desktop`) to the AUR repository.
