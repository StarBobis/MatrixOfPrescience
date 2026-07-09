import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { readFile } from "@tauri-apps/plugin-fs";

const avatarMimeTypes: Record<string, string> = {
  bmp: "image/bmp",
  gif: "image/gif",
  jpeg: "image/jpeg",
  jpg: "image/jpeg",
  png: "image/png",
  svg: "image/svg+xml",
  webp: "image/webp",
};

export function getAvatarSrc(avatar?: string) {
  const value = avatar?.trim();

  if (!value) {
    return "";
  }

  if (/^(https?:|data:|blob:|asset:)/i.test(value)) {
    return value;
  }

  if (!isTauri()) {
    return "";
  }

  return convertFileSrc(value);
}

export async function chooseLocalAvatar() {
  const selected = await open({
    directory: false,
    multiple: false,
    title: "选择头像图片",
    filters: [
      {
        name: "图片",
        extensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp", "svg"],
      },
    ],
  });

  if (typeof selected !== "string") {
    return "";
  }

  try {
    const bytes = await readFile(selected);
    const extension = selected.split(".").pop()?.toLowerCase() ?? "";
    const mimeType = avatarMimeTypes[extension] ?? "image/*";

    return `data:${mimeType};base64,${bytesToBase64(bytes)}`;
  } catch {
    return selected;
  }
}

function bytesToBase64(bytes: Uint8Array) {
  let binary = "";
  const chunkSize = 0x8000;

  for (let index = 0; index < bytes.length; index += chunkSize) {
    const chunk = bytes.subarray(index, index + chunkSize);
    binary += String.fromCharCode(...chunk);
  }

  return btoa(binary);
}
