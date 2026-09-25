import { describe, expect, it } from "vitest";
import { chartXValue, detectChartType, transformDataForChart } from "./chart-utils";
import { SqlDecimal } from "$lib/values";

describe("chart utils with decoded values", () => {
  it("treats bigint and SqlDecimal columns as numeric", () => {
    const rows = [
      [1n, new SqlDecimal("1.5")],
      [2n, new SqlDecimal("2.5")],
    ];
    expect(detectChartType(["a", "b"], rows)).toBe("scatter");
  });

  it("turns y values into numbers", () => {
    const { datasets } = transformDataForChart(
      { type: "bar", xAxis: "label", yAxis: ["n", "d"], dataScope: "page" },
      ["label", "n", "d"],
      [["x", 5n, new SqlDecimal("12.50")]],
    );
    expect(datasets.map((d) => d.data)).toEqual([[5], [12.5]]);
  });

  it("makes exact x values safe for d3", () => {
    expect(chartXValue(3n)).toBe(3);
    expect(chartXValue(new SqlDecimal("0.25"))).toBe(0.25);
    expect(chartXValue("2024-01-01")).toBe("2024-01-01");
    expect(chartXValue(7)).toBe(7);
  });
});
