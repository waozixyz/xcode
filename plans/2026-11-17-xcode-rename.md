# xcode: Sol 6.1 default, Sol 6.2 option, xcoder becomes xcode (2026-11-17)

Status lines, added as each becomes true.

- Model IDs (provider's live list on Alpha's login): Sol 6.1 = `gpt-6.1-sol`, Sol 6.2 = `gpt-6.2-sol`. Both picker-visible, 500000 context, efforts low…ultra.
- GitHub renamed: `waozixyz/xcoder` is now https://github.com/waozixyz/xcode (old URL redirects). Alpha's checkout moved in place to `~/Projects/waozixyz/xcode`, origin set to the new URL.
- Gitea is primary: https://git.waozi.xyz/waozixyz/xcode (renamed from xcoder there too). Alpha's origin points at it; every push is mirrored to GitHub by `~/Storage/gitea/scripts/mirror-github.sh` (post-receive hook plus `gitea-mirror.timer`, every 5 min). Owner, 2026-11-17: builds run on Delta, not Alpha; Delta's disk was cleaned (3.5 GB → 168 GB free) and the build runs there from Gitea.
