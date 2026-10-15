# plugins.viviplayer.app: plugin marketplace + catalog (index.json) built by scripts/build-marketplace.py.
# Dokploy app "vivi-plugins" (project "vivi") builds this file from the repo root on every push to master.
FROM python:3.12-alpine AS build
WORKDIR /src
COPY . .
RUN python3 scripts/build-marketplace.py /out
FROM nginx:1.27-alpine
COPY deploy/plugins/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /out /usr/share/nginx/html
