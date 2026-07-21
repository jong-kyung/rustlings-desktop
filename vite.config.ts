import { NuxtIconBundle } from "@nuxt/icon/vite";
import ui from "@nuxt/ui/vite";
import vue from "@vitejs/plugin-vue";
import { defineConfig, lazyPlugins, loadEnv } from "vite-plus";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, ".", "");

  return {
    plugins: lazyPlugins(() => [
      vue(),
      ui({ dts: false, router: false }),
      NuxtIconBundle({
        icons: [
          "lucide:chevron-down",
          "lucide:chevron-right",
          "lucide:circle",
          "lucide:circle-check",
          "lucide:circle-dot",
          "lucide:file-code-2",
          "lucide:loader-circle",
          "lucide:lock",
          "lucide:play",
        ],
      }),
    ]),
    staged: {
      "*": "vp check --fix",
    },
    fmt: {
      ignorePatterns: [
        ".pi-subagents/**",
        "docs/plans/**",
        "src-tauri/resources/rustlings-6.5.0/**",
      ],
    },
    lint: {
      jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
      rules: { "vite-plus/prefer-vite-plus-imports": "error" },
      options: { typeAware: true, typeCheck: true },
    },
    test: {
      include: ["src/**/*.test.ts"],
    },
    clearScreen: false,
    server: {
      port: 5173,
      strictPort: true,
      watch: {
        ignored: ["**/src-tauri/**"],
      },
    },
    envPrefix: ["VITE_", "TAURI_ENV_*"],
    build: {
      target: env.TAURI_ENV_PLATFORM == "windows" ? "chrome105" : "safari14",
      // don't minify for debug builds
      minify: env.TAURI_ENV_DEBUG === "true" ? false : "oxc",
    },
  };
});
