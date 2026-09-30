// vitest setup：测试环境默认语言钉为 zh-CN（jsdom 的 navigator.language 固定为 en-US，
// 不钉定的话现有断言中文文案的测试在抽取后会读到英文译文）。
Object.defineProperty(window.navigator, "language", { value: "zh-CN", configurable: true, writable: false });
