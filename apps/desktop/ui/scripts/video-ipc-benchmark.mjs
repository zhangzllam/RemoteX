import { performance } from "node:perf_hooks";

const scenarios = [
  { name: "720p15", width: 1280, height: 720, fps: 15 },
  { name: "1080p30", width: 1920, height: 1080, fps: 30 },
  { name: "1080p60", width: 1920, height: 1080, fps: 60 },
];

function percentile(values, quantile) {
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.min(ordered.length - 1, Math.floor(ordered.length * quantile))];
}

function runScenario(scenario, mode) {
  const frameBytes = scenario.width * scenario.height * 4;
  const source = Buffer.alloc(frameBytes, 127);
  const latencies = [];
  const rssStart = process.memoryUsage().rss;
  let rssPeak = rssStart;
  const cpuStart = process.cpuUsage();
  const started = performance.now();
  for (let frame = 0; frame < scenario.fps; frame += 1) {
    const frameStarted = performance.now();
    if (mode === "base64") {
      const encoded = source.toString("base64");
      const decoded = Buffer.from(encoded, "base64");
      new Uint8ClampedArray(decoded);
    } else {
      new Uint8ClampedArray(source);
    }
    latencies.push(performance.now() - frameStarted);
    rssPeak = Math.max(rssPeak, process.memoryUsage().rss);
  }
  const elapsedMs = performance.now() - started;
  const cpu = process.cpuUsage(cpuStart);
  const rendered = Math.min(scenario.fps, 30);
  return {
    scenario: scenario.name,
    mode,
    frameMiB: frameBytes / 1024 / 1024,
    avgLatencyMs: latencies.reduce((sum, value) => sum + value, 0) / latencies.length,
    p95LatencyMs: percentile(latencies, 0.95),
    cpuMs: (cpu.user + cpu.system) / 1000,
    rssPeakDeltaMiB: (rssPeak - rssStart) / 1024 / 1024,
    processingFps: scenario.fps / (elapsedMs / 1000),
    received: scenario.fps,
    rendered,
    dropped: scenario.fps - rendered,
  };
}

const results = scenarios.flatMap((scenario) => [runScenario(scenario, "base64"), runScenario(scenario, "bytes")]);
console.log(JSON.stringify({ generatedAt: new Date().toISOString(), node: process.version, platform: `${process.platform}-${process.arch}`, results }, null, 2));

