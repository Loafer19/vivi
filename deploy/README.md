# Deployment (Dokploy)

Dokploy project **vivi** on the VPS, two applications built from this repository (branch `master`,
autodeploy on push):

| App | Dockerfile | Domains (HTTPS, Let's Encrypt) |
|---|---|---|
| `vivi-site` | `deploy/site/Dockerfile` | `viviplayer.app`, `www.viviplayer.app` (nginx redirects www → apex) |
| `vivi-plugins` | `deploy/plugins/Dockerfile` | `plugins.viviplayer.app` (marketplace, `/index.json` catalog, `/feed.xml`) |

DNS: `A @`, `A www`, `A plugins` → the VPS. GitHub Pages (loafer19.github.io/vivi) keeps a mirror
whose canonical URLs point at viviplayer.app; the app falls back to its index.json.
Build locally: `python3 scripts/build-site.py` and `python3 scripts/build-marketplace.py`.
