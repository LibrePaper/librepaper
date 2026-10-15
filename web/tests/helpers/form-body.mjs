// The companion client sends multipart forms as encoded bytes, not as a
// FormData or Blob body, so a mocked fetch receives an ArrayBuffer and the
// boundary in the Content-Type header. Decode it the same way a server would.
export async function formOf(init) {
  return new Request("http://test.invalid/", { method: "POST", headers: init.headers, body: init.body }).formData();
}
