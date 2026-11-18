# Xcode Linux downloads

Every push to `master` builds and tests Xcode for Linux x86_64 on GitHub.
Pull requests and manual runs use the same build. Open the repository's
[Xcode Linux runs](https://github.com/waozixyz/xcode/actions/workflows/xcode-linux.yml)
and download the `xcode-linux-x86_64` artifact from a successful run.
Downloads are retained for 30 days. These builds do not create a release.

Unzip the artifact, then verify and extract its archive:

```sh
sha256sum --check xcode-linux-x86_64.tar.gz.sha256
tar -xzf xcode-linux-x86_64.tar.gz
./xcode-linux-x86_64/xcode
```

Keep the extracted directory together. Its `xcode` launcher selects the included
Ziran compiler, so custom tool programs work after moving the download. You can
link this launcher into a directory on your `PATH` under the name `xcode`.
The archive includes the CLI, tool host, Ziran compiler, standard library and
licenses. It contains no accounts or saved sessions.

Use an x86_64 Linux system with glibc 2.39 or newer (Ubuntu 24.04 or newer),
Bubblewrap, a C compiler, GNU binutils, OpenSSL 3, bzip2, zlib and Zstandard.
Private user namespaces must be available for native tool programs. On Ubuntu
24.04, install the runtime dependencies with:

```sh
sudo apt install bubblewrap build-essential libssl3t64 libbz2-1.0 zlib1g libzstd1
```

The build runs the full offline local suite before packaging and tests the
download again after extracting it to a different directory. A SHA-256 file
identifies the archive bytes; it is not a signature.
