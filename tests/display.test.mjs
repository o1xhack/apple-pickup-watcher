import assert from "node:assert/strict";
import test from "node:test";
import { loadSource } from "./load-source.mjs";
const { productLabel, colorLabel } = loadSource("src/lib/display.ts");
const { colorOptions, productsForSelection } = loadSource("src/lib/product-selection.ts");
test("双语颜色只影响显示，中文和未知颜色保留，选择仍使用原始英文与 SKU", () => {
  assert.equal(productLabel("iPhone Air 256GB Cloud White"), "iPhone Air 256GB 云白色 · Cloud White");
  assert.equal(productLabel("iPhone Duo 2TB Star White"), "iPhone Duo 2TB 星白色 · Star White");
  assert.equal(colorLabel("黑色"), "黑色");
  assert.equal(colorLabel("Future Color"), "Future Color");
  const p = {partNumber:"SKU",category:"iphone",family:"air",capacity:"256GB",color:"Cloud White",title:"iPhone Air 256GB Cloud White"};
  assert.deepEqual(colorOptions([p],"iphone",["air"],["256GB"]), [{value:"Cloud White",label:"云白色 · Cloud White"}]);
  assert.equal(productsForSelection([p],"iphone",["air"],["256GB"],["Cloud White"])[0],p);
  assert.equal(p.title,"iPhone Air 256GB Cloud White");
});
