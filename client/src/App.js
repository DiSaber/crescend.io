import React, { useMemo, useState } from "react"
import "./App.css"

const quizQuestions = [
  {
    question: "Which planet is known as the Red Planet?",
    options: ["Mars", "Venus", "Jupiter", "Mercury"],
    correct: 0,
  },
  {
    question: "What does CSS stand for?",
    options: ["Creative Style System", "Cascading Style Sheets", "Computer Style Syntax", "Colorful Styling Syntax"],
    correct: 1,
  },
  {
    question: "Which language runs in a web browser?",
    options: ["Python", "C++", "JavaScript", "Rust"],
    correct: 2,
  },
  {
    question: "What is 8 × 7?",
    options: ["42", "54", "56", "64"],
    correct: 2,
  },
]

const leaderboardSeed = [
  { name: "Maya", score: 400 },
  { name: "Leo", score: 300 },
  { name: "Ava", score: 200 },
  { name: "You", score: 0 },
]

function App() {
  const [screen, setScreen] = useState("home")
  const [playerName, setPlayerName] = useState("Player 1")
  const [roomCode, setRoomCode] = useState("KAHOOT")
  const [questionIndex, setQuestionIndex] = useState(0)
  const [selectedAnswer, setSelectedAnswer] = useState(null)
  const [score, setScore] = useState(0)
  const [correctCount, setCorrectCount] = useState(0)

  const currentQuestion = quizQuestions[questionIndex]
  const questionCount = quizQuestions.length

  const leaderboard = useMemo(() => {
    const updated = leaderboardSeed.map((entry) =>
      entry.name === "You" ? { ...entry, score } : entry
    )
    return [...updated].sort((a, b) => b.score - a.score)
  }, [score])

  const moveToNextQuestion = () => {
    if (questionIndex === questionCount - 1) {
      setScreen("results")
      return
    }

    setQuestionIndex((prev) => prev + 1)
    setSelectedAnswer(null)
  }

  const handleAnswer = (index) => {
    if (selectedAnswer !== null) return

    const isCorrect = index === currentQuestion.correct
    setSelectedAnswer(index)

    if (isCorrect) {
      setCorrectCount((prev) => prev + 1)
      setScore((prev) => prev + 100)
    }

    setTimeout(() => {
      moveToNextQuestion()
    }, 1200)
  }

  const startGame = () => {
    setScreen("game")
    setScore(0)
    setCorrectCount(0)
    setQuestionIndex(0)
    setSelectedAnswer(null)
  }

  const resetGame = () => {
    setScreen("home")
    setPlayerName("Player 1")
    setRoomCode("KAHOOT")
    setScore(0)
    setCorrectCount(0)
    setQuestionIndex(0)
    setSelectedAnswer(null)
  }

  const isAnswerCorrect = selectedAnswer === currentQuestion.correct

  return (
    <div className="app-shell">
      <div className="app-frame">
        {screen === "home" && (
          <div className="home-screen">
            <div className="brand-block">
              <span className="brand-badge">K</span>
              <p className="brand-name">Kahoot!</p>
            </div>

            <div className="entry-panel">
              <label className="field-label">Player name</label>
              <input
                className="text-input"
                value={playerName}
                onChange={(event) => setPlayerName(event.target.value || "Player 1")}
              />

              <label className="field-label">Game PIN</label>
              <input
                className="text-input"
                value={roomCode}
                onChange={(event) => setRoomCode(event.target.value.toUpperCase())}
                maxLength={6}
              />

              <button className="primary-btn" onClick={startGame}>
                Enter game
              </button>

              <div className="mini-actions">
                <button className="secondary-btn">Host a quiz</button>
                <button className="secondary-btn">Create a quiz</button>
              </div>
            </div>
          </div>
        )}

        {screen === "game" && (
          <div className="game-screen">
            <header className="topbar">
              <div>
                <p className="status-text">Room</p>
                <strong>{roomCode}</strong>
              </div>
              <div>
                <p className="status-text">Player</p>
                <strong>{playerName}</strong>
              </div>
              <div className="score-pill">{score} pts</div>
            </header>

            <div className="question-wrap">
              <div className="progress-row">
                <span>
                  {questionIndex + 1}/{questionCount}
                </span>
                <div className="progress-bar">
                  <span style={{ width: `${((questionIndex + 1) / questionCount) * 100}%` }} />
                </div>
              </div>

              <div className="question-card">
                <p className="question-tag">Question {questionIndex + 1}</p>
                <h2>{currentQuestion.question}</h2>
              </div>

              <div className="answer-grid">
                {currentQuestion.options.map((option, index) => {
                  const isSelected = selectedAnswer === index
                  const isCorrect = index === currentQuestion.correct

                  let className = "answer-card"
                  if (selectedAnswer !== null) {
                    if (isCorrect) className += " correct"
                    else if (isSelected) className += " wrong"
                  }

                  return (
                    <button
                      key={option}
                      className={className}
                      onClick={() => handleAnswer(index)}
                      disabled={selectedAnswer !== null}
                    >
                      <span className="answer-letter">{String.fromCharCode(65 + index)}</span>
                      <span>{option}</span>
                    </button>
                  )
                })}
              </div>

              {selectedAnswer !== null && (
                <div className={`feedback-banner ${isAnswerCorrect ? "success" : "error"}`}>
                  {isAnswerCorrect ? "Correct! +100 points" : `Nice try — the correct answer was ${currentQuestion.options[currentQuestion.correct]}`}
                </div>
              )}
            </div>
          </div>
        )}

        {screen === "results" && (
          <div className="results-screen">
            <div className="results-card">
              <p className="results-label">Final score</p>
              <h1>{score}</h1>
              <p>
                You answered {correctCount} out of {questionCount} questions correctly.
              </p>

              <div className="leaderboard-list">
                {leaderboard.map((entry, index) => (
                  <div key={entry.name} className={`leaderboard-item ${entry.name === playerName ? "you" : ""}`}>
                    <span className="rank">#{index + 1}</span>
                    <span>{entry.name}</span>
                    <strong>{entry.score}</strong>
                  </div>
                ))}
              </div>

              <button className="primary-btn" onClick={resetGame}>Play again</button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

export default App
