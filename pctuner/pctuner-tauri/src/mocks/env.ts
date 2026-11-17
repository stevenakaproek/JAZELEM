// "?admin=0" in the address shows the view-only state of every tab.
export const mockParams = new URLSearchParams(typeof location === "undefined" ? "" : location.search);
export const mockAdmin = mockParams.get("admin") !== "0";
