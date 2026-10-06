import Jukebox from "./components/jukebox.jsx";


export default function Home() {
  return (
    <div className="relative w-full min-h-screen flex items-center justify-center bg-gradient-to-b from-black to-[#1a0b29] text-white overflow-hidden">

    <Jukebox scale={0.9} />


      {/* BACKGROUND GLOW */}
      <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
        <div className="w-[70vw] h-[70vw] rounded-full 
          bg-[radial-gradient(circle,#6A00FF55,#000000_70%)]
          blur-[140px] opacity-70">
        </div>
      </div>

      {/* SIDE RIM LIGHTS */}
      <div className="absolute left-0 top-0 h-full w-40 
        bg-[radial-gradient(circle,#6A00FF55,transparent_70%)]
        blur-[120px] opacity-40 pointer-events-none">
      </div>

      <div className="absolute right-0 top-0 h-full w-40 
        bg-[radial-gradient(circle,#6A00FF55,transparent_70%)]
        blur-[120px] opacity-40 pointer-events-none">
      </div>

      <div className="relative z-10 flex flex-col items-center text-center px-6">

        {/* Logo / Title */}
        <h1 className="text-6xl md:text-7xl font-extrabold tracking-wide bg-clip-text text-transparent 
          bg-gradient-to-b from-white to-[#7D183B] ">
          Crescend.io
        </h1>

        {/* Tagline */}
        <p className="mt-6 text-xl md:text-2xl text-zinc-300 max-w-2xl leading-relaxed">
          The group music experience! - create sessions, vote on best music, and enjoy music together.
        </p>

        {/* Buttons */}
        <div className="mt-10 flex flex-col md:flex-row gap-6">
          <a
            href="/create"
            className="px-8 py-4 rounded-xl bg-[#5C172E] hover:bg-[#FF0048] transition font-bold text-lg"
          >
            Create Session
          </a>

          <a
            href="/join"
            className="px-8 py-4 rounded-xl bg-[#5C172E] hover:bg-[#FF0048] transition font-bold text-lg"
          >
            Join Session
          </a>
        </div>
      </div>
    </div>
  );
}