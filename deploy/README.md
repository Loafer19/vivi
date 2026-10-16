# Deployment (Dokploy)

Dokploy project **vivi** on the VPS, two applications (branch `master`, autodeploy on push):

| App | Dockerfile | Domains (HTTPS, Let's Encrypt) |
|---|---|---|
| `vivi-site` | `deploy/site/Dockerfile` (this repository) | `viviplayer.app`, `www.viviplayer.app` (nginx redirects www → apex) |
| `vivi-plugins` | `deploy/plugins/Dockerfile` in [Loafer19/vivi-plugins](https://github.com/Loafer19/vivi-plugins) | `plugins.viviplayer.app` (marketplace, `/index.json` catalog, `/feed.xml`) |

DNS: `A @`, `A www`, `A plugins` → the VPS. GitHub Pages (loafer19.github.io/vivi) keeps a mirror
whose canonical URLs point at viviplayer.app. The plugin catalog fallback is the vivi-plugins
Pages site (loafer19.github.io/vivi-plugins/index.json).
Build locally: `python3 scripts/build-site.py` (downloads the catalog from vivi-plugins, or set
`VIVI_PLUGINS_INDEX=/path/to/index.json`).
