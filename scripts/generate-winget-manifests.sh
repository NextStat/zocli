#!/bin/sh
set -eu

VERSION=""
BASE_URL=""
OUTPUT_DIR=""
SHA_WINDOWS_X64=""

usage() {
    cat <<'EOF'
Generate a winget manifest bundle for a released zocli version.

Usage:
  generate-winget-manifests.sh \
    --version VERSION \
    --base-url URL \
    --output-dir DIR \
    --sha256-windows-x64 SHA256
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            VERSION="$2"
            shift 2
            ;;
        --base-url)
            BASE_URL="$2"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --sha256-windows-x64)
            SHA_WINDOWS_X64="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [ -z "$VERSION" ] || [ -z "$BASE_URL" ] || [ -z "$OUTPUT_DIR" ] || [ -z "$SHA_WINDOWS_X64" ]; then
    usage >&2
    exit 1
fi

package_id="NextStat.zocli"
manifest_root="${OUTPUT_DIR}/${package_id}"
mkdir -p "$manifest_root"

cat > "${manifest_root}/${package_id}.yaml" <<EOF
PackageIdentifier: ${package_id}
PackageVersion: ${VERSION}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.6.0
EOF

cat > "${manifest_root}/${package_id}.installer.yaml" <<EOF
PackageIdentifier: ${package_id}
PackageVersion: ${VERSION}
Platform:
  - Windows.Desktop
InstallModes:
  - interactive
  - silent
  - silentWithProgress
Installers:
  - Architecture: x64
    InstallerType: zip
    NestedInstallerType: portable
    NestedInstallerFiles:
      - RelativeFilePath: zocli.exe
        PortableCommandAlias: zocli
    InstallerUrl: ${BASE_URL}/zocli-x86_64-pc-windows-msvc.zip
    InstallerSha256: ${SHA_WINDOWS_X64}
ManifestType: installer
ManifestVersion: 1.6.0
EOF

cat > "${manifest_root}/${package_id}.locale.en-US.yaml" <<EOF
PackageIdentifier: ${package_id}
PackageVersion: ${VERSION}
PackageLocale: en-US
Publisher: NextStat
PublisherUrl: https://github.com/NextStat
PublisherSupportUrl: https://github.com/NextStat/zocli/issues
Author: NextStat
PackageName: zocli
PackageUrl: https://github.com/NextStat/zocli
License: MIT
LicenseUrl: https://github.com/NextStat/zocli/blob/v${VERSION}/LICENSE
ShortDescription: Zoho Mail, Calendar, and WorkDrive CLI for humans and AI agents
Description: zocli is an open-source command-line interface for working with Zoho Mail, Zoho Calendar, and Zoho WorkDrive from the terminal and from AI agents.
Moniker: zocli
Tags:
  - cli
  - zoho
  - mail
  - calendar
  - workdrive
ManifestType: defaultLocale
ManifestVersion: 1.6.0
EOF
