// @vitest-environment happy-dom

import ui from "@nuxt/ui/vue-plugin";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { createApp, nextTick, type App as VueApp } from "vue";
import App from "../App.vue";

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

  it("provides one escaped overlay and restores trigger focus", async () => {
    const host = document.createElement("div");
    document.body.append(host);

    const app = createApp(App).use(ui);
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
