import { describe, it, expect } from "bun:test";
import { render } from "@testing-library/svelte";
import PulseMark from "@/components/PulseMark.svelte";

describe("PulseMark", () => {
    it("renders an svg mark sized to the size prop", () => {
        const { container } = render(PulseMark, { props: { size: 40 } });
        const svg = container.querySelector("svg.pulse-mark");
        expect(svg).not.toBeNull();
        expect(svg?.getAttribute("width")).toBe("40");
        expect(svg?.getAttribute("height")).toBe("40");
    });

    it("draws only the P glyph when showPulse is false", () => {
        const { container } = render(PulseMark, {
            props: { showPulse: false },
        });
        expect(container.querySelectorAll("path").length).toBe(1);
    });

    it("draws the P glyph plus the pulse line when showPulse is true", () => {
        const { container } = render(PulseMark, { props: { showPulse: true } });
        expect(container.querySelectorAll("path").length).toBe(2);
    });
});
