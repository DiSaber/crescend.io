export default function Jukebox({ scale = 1 }) {
  return (
    <div
      style={{
        transform: `scale(${scale})`,
        transformOrigin: "top center",
      }}
    >
      <div className="w-225 flex items-center justify-center bg-gradient-to-b from-black to-red-800 rounded-b-[80px]">

<div className="absolute bottom-0 left-0 w-full h-[40px] rounded-b-[80px] overflow-hidden pointer-events-none"></div>
        {/* OUTER BORDER WRAPPER */}
        <div className="relative w-[85vw] max-w-4xl">

          {/* Pulsing Outer Border */}
          <div className="absolute -inset-2 rounded-[90px] border-[8px] border-transparent animate-[multiPulse_3s_linear_infinite] pointer-events-none"></div>

          {/* JUKEBOX CONTAINER */}
          <div className="relative p-10 rounded-[80px] bg-gradient-to-b from-black to-red-800 shadow-[inset_0_0_20px_rgba(255,255,255,0.3),0_0_50px_rgba(255,200,0,0.5)] flex flex-col items-center">

            {/* JUKEBOX CONTENT */}
            <div className="relative z-10 w-full flex flex-col items-center">

              {/* ARCHED TOP */}
              <div className="relative w-full h-[40vh] rounded-t-[80px] bg-gradient-to-b from-black to-red-700 flex flex-col items-center justify-center shadow-inner animate-[flicker_2s_ease-in-out_infinite]">

                <div className="absolute inset-3 rounded-t-[80px] bg-[linear-gradient(180deg,#FF7A00,#FF3300,#FF0055)] opacity-60 blur-xl pointer-events-none"></div>

                {/* Inner Glow Chamber */}
                <div className="absolute top-10 w-[70%] h-[60%] rounded-full bg-[radial-gradient(circle,#FF1908,#FF0055,#3300FF)] opacity-40 blur-2xl pointer-events-none"></div>


                {/* Vinyl */}
                <div className="relative w-65 h-65 rounded-full mt-6 flex items-center justify-center border-4 border-zinc-700 bg-[radial-gradient(circle,_#000_0%,_#111_10%,_#000_20%,_#111_30%,_#000_40%,_#111_50%,_#000_60%,_#111_70%,_#000_80%,_#111_90%,_#000_100%)] animate-[spin_4s_linear_infinite,wobble_2s_ease-in-out_infinite] z-10">

                  {/* Glossy Reflection */}
                  <div className="absolute inset-0 rounded-full pointer-events-none bg-[radial-gradient(circle_at_30%_30%,rgba(255,255,255,0.25),rgba(255,255,255,0)_40%)]"></div>

                  {/* Shine Sweep */}
                  <div className="absolute inset-0 rounded-full pointer-events-none bg-[linear-gradient(120deg,transparent_40%,rgba(255,255,255,0.4)_50%,transparent_60%)] bg-[length:200%_200%] animate-[shine_3s_linear_infinite]"></div>

                  {/* Inner Vinyl Label */}
                  <div className="relative w-24 h-24 bg-red-600 rounded-full border-4 border-black flex items-center justify-center">
                    <div className="absolute w-4 h-4 bg-black rounded-full"></div>
                  </div>
                </div>
              </div>

              {/* Neon Tubes */}
              <div className="flex justify-center gap-6 my-10">
                <div className="w-6 h-28 bg-red-600 rounded-full shadow-[0_0_25px_#FF0055] animate-pulse [animation-delay:0s]"></div>
                <div className="w-6 h-28 bg-blue-600 rounded-full shadow-[0_0_25px_#00A8FF] animate-pulse [animation-delay:0.3s]"></div>
                <div className="w-6 h-28 bg-[#00FFFF] rounded-full shadow-[0_0_25px_#00FFFF] animate-pulse [animation-delay:0.6s]"></div>
                <div className="w-6 h-28 bg-purple-500 rounded-full shadow-[0_0_25px_#B200FF] animate-pulse [animation-delay:0.9s]"></div>
                <div className="w-6 h-28 bg-[#F52791] rounded-full shadow-[0_0_25px_#F52791] animate-pulse [animation-delay:1.2s]"></div>
              </div>

              {/* Grill */}
              <div className="relative w-full mb-10">

                {/* Outer Border (pulsing) */}
                <div className="absolute -inset-1 rounded-xl border-[10px] border-transparent animate-[multiPulse_3s_linear_infinite] pointer-events-none"></div>

                {/* Inner Grill Content */}
                <div className="relative bg-zinc-800 rounded-xl p-6">
                  <div className="grid grid-cols-6 gap-2">
                    {[...Array(24)].map((_, i) => (
                      <div key={i} className="w-30 h-20 bg-zinc-700 rounded-sm"></div>
                    ))}
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
