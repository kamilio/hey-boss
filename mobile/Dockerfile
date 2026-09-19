FROM node:22-alpine AS build
WORKDIR /app
COPY mobile/package*.json ./
RUN npm ci
COPY mobile/ .
COPY src/issues/web/ /src/issues/web/
RUN npm run build
FROM node:22-alpine
ENV NODE_ENV=production PORT=8787 DATA_DIR=/data
WORKDIR /app
COPY mobile/package*.json ./
RUN npm ci --omit=dev
COPY --from=build /app/dist ./dist
COPY mobile/server/index.mjs mobile/server/store.mjs mobile/server/markdown-text.mjs ./server/
EXPOSE 8787
CMD ["node","server/index.mjs"]
