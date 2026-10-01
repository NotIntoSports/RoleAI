import { describe, expect, it } from "vitest";
import { routeIds, parseHash, formatHash, routeLabel, routeDesignRef, routeCapabilities, isRouteId, type RouteId } from "./routes";

describe("routes", () => {
  it("routeIds is a fixed ordered tuple of 6 pages", () => {
    expect(routeIds).toEqual(["workspace", "livestream", "materials", "records", "services", "settings"]);
  });

  describe("parseHash", () => {
    it("empty string falls back to workspace", () => {
      expect(parseHash("")).toBe("workspace");
    });
    it("#/ falls back to workspace", () => {
      expect(parseHash("#/")).toBe("workspace");
    });
    it("parses each valid route", () => {
      expect(parseHash("#/workspace")).toBe("workspace");
      expect(parseHash("#/materials")).toBe("materials");
      expect(parseHash("#/records")).toBe("records");
      expect(parseHash("#/services")).toBe("services");
      expect(parseHash("#/settings")).toBe("settings");
    });
    it("unknown path falls back to workspace", () => {
      expect(parseHash("#/unknown")).toBe("workspace");
      expect(parseHash("#/login")).toBe("workspace");
    });
    it("tolerates query parameters", () => {
      expect(parseHash("#/settings?x=1")).toBe("settings");
      expect(parseHash("#/records?page=2&limit=10")).toBe("records");
    });
    it("handles wouter location format (no # prefix)", () => {
      expect(parseHash("/materials")).toBe("materials");
      expect(parseHash("/")).toBe("workspace");
      expect(parseHash("/services")).toBe("services");
    });
  });

  describe("formatHash", () => {
    it("formats each route id", () => {
      expect(formatHash("workspace")).toBe("#/workspace");
      expect(formatHash("records")).toBe("#/records");
      expect(formatHash("settings")).toBe("#/settings");
    });
  });

  describe("routeLabel", () => {
    it("returns Chinese labels", () => {
      expect(routeLabel("workspace")).toBe("工作台");
      expect(routeLabel("materials")).toBe("资料");
      expect(routeLabel("records")).toBe("记录");
      expect(routeLabel("services")).toBe("服务");
      expect(routeLabel("settings")).toBe("设置");
    });
  });

  describe("routeDesignRef", () => {
    it("returns design spec section numbers", () => {
      expect(routeDesignRef("workspace")).toBe("6.1");
      expect(routeDesignRef("materials")).toBe("6.2");
      expect(routeDesignRef("records")).toBe("6.3");
      expect(routeDesignRef("services")).toBe("6.4");
      expect(routeDesignRef("settings")).toBe("6.5");
    });
  });

  describe("routeCapabilities", () => {
    it("workspace includes session and subtitle capabilities", () => {
      const caps = routeCapabilities("workspace");
      expect(caps.length).toBeGreaterThan(0);
      expect(caps.some(c => c.includes("会话"))).toBe(true);
      expect(caps.some(c => c.includes("字幕"))).toBe(true);
    });
    it("materials includes resume and knowledge capabilities", () => {
      const caps = routeCapabilities("materials");
      expect(caps.length).toBeGreaterThan(0);
      expect(caps.some(c => c.includes("简历"))).toBe(true);
    });
    it("records includes history and export capabilities", () => {
      const caps = routeCapabilities("records");
      expect(caps.length).toBeGreaterThan(0);
      expect(caps.some(c => c.includes("记录"))).toBe(true);
    });
    it("services includes provider and speech capabilities", () => {
      const caps = routeCapabilities("services");
      expect(caps.length).toBeGreaterThan(0);
      expect(caps.some(c => c.includes("模型"))).toBe(true);
    });
    it("settings includes diagnostics and config capabilities", () => {
      const caps = routeCapabilities("settings");
      expect(caps.length).toBeGreaterThan(0);
      expect(caps.some(c => c.includes("诊断"))).toBe(true);
    });
    it("all routes have non-empty capabilities", () => {
      for (const id of routeIds) {
        expect(routeCapabilities(id).length).toBeGreaterThan(0);
      }
    });
  });

  describe("isRouteId", () => {
    it("returns true for valid route ids", () => {
      expect(isRouteId("workspace")).toBe(true);
      expect(isRouteId("materials")).toBe(true);
      expect(isRouteId("records")).toBe(true);
      expect(isRouteId("services")).toBe(true);
      expect(isRouteId("settings")).toBe(true);
    });
    it("returns false for invalid values", () => {
      expect(isRouteId("login")).toBe(false);
      expect(isRouteId("stage")).toBe(false);
      expect(isRouteId("")).toBe(false);
      expect(isRouteId(null)).toBe(false);
      expect(isRouteId(undefined)).toBe(false);
      expect(isRouteId(123)).toBe(false);
    });
  });
});
