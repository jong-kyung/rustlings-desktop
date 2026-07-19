// @vitest-environment happy-dom

import ui from "@nuxt/ui/vue-plugin";
import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { createApp, nextTick, type App as VueApp } from "vue";
import App from "../App.vue";
import {
  DEFAULT_SIDEBAR_WIDTH,
  MAX_SIDEBAR_WIDTH,
  MIN_SIDEBAR_WIDTH,
  SIDEBAR_WIDTH_STORAGE_KEY,
  persistSidebarWidth,
  readSidebarWidth,
} from "../lib/sidebarWidth";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    onCloseRequested: vi.fn(async () => () => undefined),
    destroy: vi.fn(async () => undefined),
  })),
}));

vi.mock("../composables/useLearningSession", async () => {
  const { ref } = await import("vue");
  return {
    sanitizeDisplayText: (value: string) => value,
    useLearningSession: () => ({
      loading: ref(false),
      snapshot: ref({
        exercises: [
          {
            id: "intro1",
            sourcePath: "exercises/00_intro/intro1.rs",
            status: "current",
            revision: 0,
          },
        ],
        selected: "intro1",
        solutionAvailable: false,
        curriculumComplete: false,
        preflight: { ready: false, message: "Toolchain unavailable" },
        readme: "Lesson",
        sourceDigest: "digest",
      }),
      source: ref("fn main() {}"),
      solution: ref(undefined),
      hint: ref(undefined),
      runResult: ref(undefined),
      diagnostics: ref(undefined),
      error: ref(undefined),
      saveError: ref(undefined),
      dirty: ref(false),
      saving: ref(false),
      running: ref(false),
      navigating: ref(false),
      revealingSolution: ref(false),
      retryingPreflight: ref(false),
      canCancel: ref(false),
      cancelling: ref(false),
      initialize: vi.fn(async () => undefined),
      flushSaves: vi.fn(async () => true),
      revealSolution: vi.fn(async () => false),
      selectExercise: vi.fn(),
      editSource: vi.fn(),
      run: vi.fn(),
      cancel: vi.fn(),
      retryPreflight: vi.fn(),
      revealHint: vi.fn(),
    }),
  };
});

const mountedApps: VueApp[] = [];

async function mountApp() {
  const host = document.createElement("div");
  document.body.append(host);
  const app = createApp(App).use(ui);
  mountedApps.push(app);
  app.mount(host);
  await nextTick();
  return {
    shell: host.querySelector<HTMLElement>(".learning-shell")!,
    separator: host.querySelector<HTMLElement>('[role="separator"]')!,
  };
}

beforeEach(() => localStorage.clear());

afterEach(() => {
  for (const app of mountedApps.splice(0)) app.unmount();
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("sidebar width storage", () => {
  it.each([
    [null, DEFAULT_SIDEBAR_WIDTH],
    ["not-a-number", DEFAULT_SIDEBAR_WIDTH],
    ["", DEFAULT_SIDEBAR_WIDTH],
    ["100", MIN_SIDEBAR_WIDTH],
    ["301", 301],
    ["999", MAX_SIDEBAR_WIDTH],
  ])("loads %s as %i pixels", (stored, expected) => {
    if (stored !== null) localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, stored);
    expect(readSidebarWidth()).toBe(expected);
  });

  it("ignores unavailable storage for reads and writes", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(readSidebarWidth()).toBe(DEFAULT_SIDEBAR_WIDTH);

    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(() => persistSidebarWidth(300)).not.toThrow();
  });
});

describe("sidebar separator", () => {
  it("clamps pointer dragging continuously and persists only on release or cancel", async () => {
    const { shell, separator } = await mountApp();
    shell.getBoundingClientRect = () => ({ left: 20 }) as DOMRect;
    const setPointerCapture = vi.fn();
    separator.setPointerCapture = setPointerCapture;

    separator.dispatchEvent(
      new PointerEvent("pointerdown", { bubbles: true, clientX: 292, pointerId: 1 }),
    );
    separator.dispatchEvent(
      new PointerEvent("pointermove", { bubbles: true, clientX: 1_000, pointerId: 1 }),
    );
    await nextTick();

    expect(setPointerCapture).toHaveBeenCalledWith(1);
    expect(separator.getAttribute("aria-valuenow")).toBe(String(MAX_SIDEBAR_WIDTH));
    expect(shell.style.getPropertyValue("--sidebar-width")).toBe(`${MAX_SIDEBAR_WIDTH}px`);
    expect(localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY)).toBeNull();

    separator.dispatchEvent(
      new PointerEvent("pointerup", { bubbles: true, clientX: 1_000, pointerId: 1 }),
    );
    expect(localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY)).toBe(String(MAX_SIDEBAR_WIDTH));

    separator.dispatchEvent(
      new PointerEvent("pointerdown", { bubbles: true, clientX: 404, pointerId: 2 }),
    );
    separator.dispatchEvent(
      new PointerEvent("pointermove", { bubbles: true, clientX: -100, pointerId: 2 }),
    );
    separator.dispatchEvent(new PointerEvent("pointercancel", { bubbles: true, pointerId: 2 }));
    await nextTick();

    expect(separator.getAttribute("aria-valuenow")).toBe(String(MIN_SIDEBAR_WIDTH));
    expect(localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY)).toBe(String(MIN_SIDEBAR_WIDTH));
  });

  it("supports keyboard steps and bounds with complete separator ARIA", async () => {
    const { shell, separator } = await mountApp();

    expect(separator.tabIndex).toBe(0);
    expect(separator.getAttribute("aria-orientation")).toBe("vertical");
    expect(separator.getAttribute("aria-valuemin")).toBe(String(MIN_SIDEBAR_WIDTH));
    expect(separator.getAttribute("aria-valuemax")).toBe(String(MAX_SIDEBAR_WIDTH));
    expect(separator.getAttribute("aria-valuenow")).toBe(String(DEFAULT_SIDEBAR_WIDTH));

    const left = new KeyboardEvent("keydown", {
      key: "ArrowLeft",
      bubbles: true,
      cancelable: true,
    });
    separator.dispatchEvent(left);
    await nextTick();
    expect(left.defaultPrevented).toBe(true);
    expect(separator.getAttribute("aria-valuenow")).toBe("264");
    expect(localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY)).toBe("264");

    for (const [key, width] of [
      ["Home", MIN_SIDEBAR_WIDTH],
      ["End", MAX_SIDEBAR_WIDTH],
    ] as const) {
      const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
      separator.dispatchEvent(event);
      await nextTick();
      expect(event.defaultPrevented).toBe(true);
      expect(separator.getAttribute("aria-valuenow")).toBe(String(width));
      expect(shell.style.getPropertyValue("--sidebar-width")).toBe(`${width}px`);
      expect(localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY)).toBe(String(width));
    }
  });
});
