# https://docs.docker.com/build/attestations/sbom/#scan-build-context
ARG BUILDKIT_SBOM_SCAN_CONTEXT=true

FROM node:23-alpine3.20 AS base

ENV NODE_ENV=development
ENV NITRO_PRESET=node-server
ENV NITRO_PORT=3000
ENV NITRO_HOST=0.0.0.0

ARG GIT_COMMIT_SHA
ENV GIT_COMMIT_SHA=${GIT_COMMIT_SHA}
ARG GIT_COMMIT_MESSAGE
ENV GIT_COMMIT_MESSAGE=${GIT_COMMIT_MESSAGE}

WORKDIR /app

FROM base AS build-stage

ENV PNPM_HOME="/pnpm"
ENV PATH="$PNPM_HOME:$PATH"
RUN corepack enable
COPY package.json ./
COPY pnpm-lock.yaml ./
RUN NODE_ENV=development pnpm install --frozen-lockfile

COPY tsconfig.json .
COPY eslint.config.mjs .
COPY ./*.ts .
COPY --link public public
COPY --link content content
COPY --link app app
RUN pnpm run build && test -f /app/.output/server/index.mjs

FROM base AS production-stage
RUN apk update --no-progress --quiet && \
    apk add --no-cache --no-progress --quiet curl

COPY --from=build-stage /app/.output /app/.output
COPY --from=build-stage /app/node_modules /app/node_modules
COPY --from=build-stage /app/.nuxt /app/.nuxt

EXPOSE 3000

HEALTHCHECK CMD curl --fail localhost:3000/health || exit 1
CMD ["node", "/app/.output/server/index.mjs"]
