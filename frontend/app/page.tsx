import Jukebox from "./components/jukebox.jsx";

export default function Home() {
  return (
    <div className="min-h-screen bg-[#05EEFF]} text-white flex flex-col">

      {/* Navigation Bar */}
      <nav className="w-full py-4 px-8 border-b border-zinc-800 flex items-center">
        <div className="flex-1"></div>

        <div className="flex gap-6 text-lg flex-1 justify-end">
          <a href="/" className="hover:text-zinc-400 transition">Home</a>
          <a href="/join" className="hover:text-zinc-400 transition">Join</a>
          <a href="/create" className="hover:text-zinc-400 transition">Create</a>
          <a href="/session" className="hover:text-zinc-400 transition">Session</a>
        </div>
      </nav>

      {/* Main Content */}
      <main className="flex flex-col flex-1 items-center justify-center px-6 gap-10">

        {/* Jukebox Component */}
        <Jukebox />

      </main>

    </div>
  );
}

