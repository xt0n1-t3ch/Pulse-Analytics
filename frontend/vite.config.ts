import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig(() => {
  const bridgeToken = process.env.PULSE_DEV_BRIDGE_TOKEN;
  const uiPort = Number(process.env.PULSE_DEV_PORT ?? 1420);
  const bridgePort = Number(process.env.PULSE_DEV_BRIDGE_PORT ?? 1421);
  if (![uiPort, bridgePort].every((port) => Number.isInteger(port) && port >= 1024 && port <= 65535)
      || uiPort === bridgePort) {
    throw new Error("Pulse development ports must be distinct integers between 1024 and 65535.");
  }

  return {
    plugins: [svelte()],
    clearScreen: false,
    server: {
      port: uiPort,
      strictPort: true,
      proxy: bridgeToken
        ? {
            "/__pulse_api": {
              target: `http://127.0.0.1:${bridgePort}`,
              changeOrigin: false,
              headers: { Authorization: `Bearer ${bridgeToken}`, cookie: "" },
              rewrite: () => "/invoke",
              configure(proxy) {
                proxy.on("proxyReq", (request) => {
                  // Browser profile cookies are unrelated to the authenticated
                  // loopback contract and can exceed the bridge's bounded HTTP
                  // header limit. Never forward them across this boundary.
                  request.removeHeader("cookie");
                  request.setHeader("Authorization", `Bearer ${bridgeToken}`);
                });
              },
            },
          }
        : undefined,
    },
    build: {
      target: "esnext",
      outDir: "dist",
    },
  };
});
