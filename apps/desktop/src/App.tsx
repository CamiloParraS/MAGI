import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

function App() {
  const [message, setMessage] = useState("...");

  useEffect(() => {
    invoke<string>("ping").then(setMessage).catch(console.error);
  }, []);

  return (
    <main className="flex min-h-screen items-center justify-center bg-neutral-950 text-neutral-100">
      <p data-testid="ping-result" className="text-lg">
        magi-core says: {message}
      </p>
    </main>
  );
}

export default App;
