# viviplayer.app: docs site built by scripts/build-site.py, served by nginx.
# Dokploy app "vivi-site" (project "vivi") builds this file from the repo root on every push to master.
FROM python:3.12-alpine AS build
WORKDIR /src
COPY . .
RUN python3 scripts/build-site.py /out
FROM nginx:1.27-alpine
COPY deploy/site/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /out /usr/share/nginx/html
