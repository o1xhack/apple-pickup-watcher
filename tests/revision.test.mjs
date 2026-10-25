import assert from "node:assert/strict";
import test from "node:test";
import {loadSource} from "./load-source.mjs";
const {targetRevision} = loadSource("src/lib/revision.ts");
test("desktop and Rust use the same target revision including defaults", async()=>{
 const target={locale:"en_US",storeNumber:"R105",partNumber:"SKU",storeTitle:"Oakridge",productName:"iPhone"};
 assert.equal(await targetRevision([target]), "0c4aee7dc4077ded9c809d20c791b7b71461e9c2e1836dbe5cf9acccbfb45e30");
 assert.equal(await targetRevision([{...target,criticalAlert:false,pickupLocation:undefined}]),await targetRevision([target]));
 assert.notEqual(await targetRevision([{...target,criticalAlert:true}]),await targetRevision([target]));
 const second={...target,storeNumber:"R039"};
 assert.equal(await targetRevision([target,second]),await targetRevision([second,target]));
});
