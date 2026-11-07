import { plugin } from "bun";
import { vi as bunVi, type Mock } from "bun:test";

export const vi = bunVi as Omit<typeof bunVi, "mock"> & {
    hoisted: <T>(factory: () => T) => T;
    mocked: <T extends (...args: any[]) => any>(value: T) => Mock<T>;
    stubGlobal: (key: string, value: unknown) => void;
    unstubAllGlobals: () => void;
    mock: (
        id: string,
        factory: (original: <T>() => Promise<T>) => unknown,
    ) => void;
};

// Preloads run before test modules; factories can be evaluated immediately.
const viCompat = vi as unknown as {
    hoisted?: <T>(factory: () => T) => T;
    mocked?: <T>(value: T) => T;
    stubGlobal?: (key: string, value: unknown) => void;
    unstubAllGlobals?: () => void;
};
viCompat.hoisted ??= (factory) => factory();
viCompat.mocked ??= (value) => value;
const globals = new Map<string, PropertyDescriptor | undefined>();
viCompat.stubGlobal ??= (key, value) => {
    if (!globals.has(key))
        globals.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
    Object.defineProperty(globalThis, key, {
        configurable: true,
        writable: true,
        value,
    });
};
viCompat.unstubAllGlobals ??= () => {
    for (const [key, descriptor] of globals) {
        if (descriptor) Object.defineProperty(globalThis, key, descriptor);
        else Reflect.deleteProperty(globalThis, key);
    }
    globals.clear();
};
import { GlobalRegistrator } from "@happy-dom/global-registrator";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

GlobalRegistrator.register({ url: "http://localhost/" });

const svelteRoot = dirname(
    Bun.resolveSync("svelte/package.json", import.meta.dir),
);
const svelteExports: Record<string, Record<string, string> | string> =
    JSON.parse(readFileSync(join(svelteRoot, "package.json"), "utf8")).exports;

await plugin({
    name: "svelte-client",
    async setup(build) {
        const { compile, compileModule } = await import("svelte/compiler");
        build.onResolve({ filter: /^svelte(\/.*)?$/ }, ({ path }) => {
            const key =
                path === "svelte" ? "." : `./${path.slice("svelte/".length)}`;
            const entry = svelteExports[key];
            if (entry && typeof entry === "object" && entry.browser)
                return { path: join(svelteRoot, entry.browser) };
            return undefined;
        });
        const srcRoot = join(import.meta.dir, "../frontend/src");
        build.onResolve({ filter: /^@\// }, ({ path }) => ({
            path: Bun.resolveSync(`./${path.slice(2)}`, srcRoot + "/"),
        }));
        build.onLoad(
            { filter: /[\\/]frontend[\\/]src[\\/]lib[\\/]api\.ts$/ },
            async ({ path }) => ({
                contents: (await Bun.file(path).text()).replaceAll(
                    "import.meta.env.DEV",
                    "true",
                ),
                loader: "ts",
            }),
        );
        build.onLoad({ filter: /\.svelte$/ }, async ({ path }) => {
            const source = await Bun.file(path).text();
            const { js } = compile(source, {
                filename: path,
                generate: "client",
                dev: true,
                css: "injected",
            });
            return {
                contents: js.code.replaceAll("import.meta.env.DEV", "true"),
                loader: "js",
            };
        });
        build.onLoad({ filter: /\.svelte\.(js|ts)$/ }, async ({ path }) => {
            const source = await Bun.file(path).text();
            const { js } = compileModule(source, {
                filename: path,
                generate: "client",
                dev: true,
            });
            return {
                contents: js.code.replaceAll("import.meta.env.DEV", "true"),
                loader: "ts",
            };
        });
    },
});
