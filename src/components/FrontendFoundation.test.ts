// @vitest-environment happy-dom

import tauriConfigSource from "../../src-tauri/tauri.conf.json?raw";
import viteConfigSource from "../../vite.config.ts?raw";
import appSource from "../App.vue?raw";
import UApp from "@nuxt/ui/components/App.vue";
import UButton from "@nuxt/ui/components/Button.vue";
import UModal from "@nuxt/ui/components/Modal.vue";
import ui from "@nuxt/ui/vue-plugin";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { createApp, h, nextTick, type App as VueApp } from "vue";

const mountedApps: VueApp[] = [];

async function settleOverlay() {
  await nextTick();
  await new Promise((resolve) => setTimeout(resolve));
}

afterEach(() => {
  for (const app of mountedApps.splice(0)) app.unmount();
  document.body.replaceChildren();
});

describe("frontend foundation", () => {
  it("resolves the standalone Nuxt UI package exports", () => {
    expect(import.meta.resolve("@nuxt/ui/vite")).toContain("@nuxt/ui/dist/vite.mjs");
    expect(typeof ui.install).toBe("function");
  });

  it("keeps the BigInt-capable WebKit and macOS bundle floors aligned", () => {
    expect(viteConfigSource).toContain('? "chrome105" : "safari14"');
    expect(JSON.parse(tauriConfigSource).bundle.macOS.minimumSystemVersion).toBe("11.0");
  });

  it("flushes dirty source before Tauri closes the window", () => {
    expect(appSource).toContain("onCloseRequested");
    expect(appSource).toContain("event.preventDefault()");
    expect(appSource).toContain("await session.flushSaves()");
  });

  it("provides one escaped overlay and restores trigger focus", async () => {
    const host = document.createElement("div");
    document.body.append(host);

    const plainTextFixture = '<script>alert("escaped")<' + "/script>";
    const app = createApp({
      setup: () => () =>
        h(UApp, { toaster: null }, () =>
          h(
            UModal,
            { title: "Local text preview", close: false, transition: false },
            {
              default: () => h(UButton, { type: "button", label: "Test overlay" }),
              body: () => h("p", { textContent: plainTextFixture }),
            },
          ),
        ),
    }).use(ui);
    mountedApps.push(app);
    app.mount(host);

    const trigger = document.querySelector<HTMLButtonElement>("button");
    expect(trigger).not.toBeNull();
    trigger?.focus();
    trigger?.click();
    await settleOverlay();

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog?.textContent).toContain('<script>alert("escaped")</script>');
    expect(dialog?.querySelector("script")).toBeNull();

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settleOverlay();

    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });
});
