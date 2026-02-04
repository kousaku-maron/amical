export const showOAuthNotImplemented = (message?: string) => {
  const text = message?.trim() || "OAuth は未実装です。";
  if (typeof window !== "undefined") {
    window.alert(text);
  } else {
    console.warn(text);
  }
};
