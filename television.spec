%define name television
%define version 0.14.4
%define release 1%{?dist}

Summary:  The revolution will (not) be televised
Name:     %{name}
Version:  %{version}
Release:  %{release}
License:  MIT License
URL:      https://github.com/alexpasmantier/television
Source0:  https://github.com/alexpasmantier/television/archive/refs/tags/%{version}.tar.gz

%define debug_package %{nil}
%undefine _package_note_file

BuildRequires: curl
BuildRequires: gcc
BuildRequires: gzip

%description
A cross-platform, fast and extensible general purpose fuzzy finder TUI.

%prep
%setup -q

%build
# Install Rust using curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
export PATH="$PATH:$HOME/.cargo/bin"
cargo build --release
strip target/release/tv

%install
# Create the necessary directory structure in the buildroot
mkdir -p %{buildroot}/bin
mkdir -p %{buildroot}/usr/share/man/man1

# Copy the binary to /bin in the buildroot
install -m 755 target/release/tv %{buildroot}/bin/
mkdir -p %{buildroot}/usr/share/doc/%{name}
mkdir -p %{buildroot}/usr/share/licenses/%{name}
install -m 644 LICENSE %{buildroot}/usr/share/licenses/%{name}/
install -m 644 README.md %{buildroot}/usr/share/doc/%{name}/
gzip man/tv.1
install -m 644 man/tv.1.gz %{buildroot}/usr/share/man/man1/

%files
/bin/tv
/usr/share/licenses/%{name}/LICENSE
/usr/share/doc/%{name}/README.md
/usr/share/man/man1/tv.1.gz
