{-# LANGUAGE TypeApplications #-}
{-# LANGUAGE TypeFamilies #-}

-- All numerical and cryptographic decisions come from the pinned ledger packages.
module Main (main) where

import Cardano.Crypto.Hash (hashFromBytes)
import qualified Cardano.Crypto.Seed as Seed
import qualified Cardano.Crypto.VRF as VRF
import Cardano.Ledger.BaseTypes
  ( ActiveSlotCoeff, FixedPoint, Nonce (..), PositiveUnitInterval, SlotNo (..)
  , activeSlotLog, boundRational, mkActiveSlotCoeff )
import Cardano.Ledger.NonIntegral (CompareResult (..), exp', ln', taylorExpCmp)
import Cardano.Protocol.Crypto (Crypto (VRF), StandardCrypto)
import qualified Cardano.Protocol.Praos.VRF as Praos
import Cardano.Protocol.TPraos.BlockHeader
  ( assertBoundedNatural, checkLeaderNatValue, checkLeaderValue, mkSeed, seedL )
import Control.Monad (forM_, unless)
import qualified Data.ByteString as BS
import Data.Char (digitToInt, isHexDigit)
import Data.Fixed (Fixed (MkFixed))
import Data.Proxy (Proxy (..))
import Data.Ratio ((%))
import Numeric (showHex)
import System.Environment (getArgs)
import System.IO (isEOF)

scale :: Integer
scale = 10 ^ (34 :: Int)

raw :: FixedPoint -> String
raw (MkFixed n) = show n

fixed :: Integer -> FixedPoint
fixed = MkFixed

format :: FixedPoint -> String
format (MkFixed n) = sign ++ show whole ++ "." ++ replicate (34 - length fraction) '0' ++ fraction
  where
    sign = if n < 0 then "-" else ""
    (whole, remainder) = abs n `divMod` scale
    fraction = show remainder

comparison :: CompareResult FixedPoint -> [String]
comparison (ABOVE _ n) = ["ABOVE", show n]
comparison (BELOW _ n) = ["BELOW", show n]
comparison (MaxReached n) = ["MAX_REACHED", show n]

coefficient :: Integer -> Integer -> ActiveSlotCoeff
coefficient numerator denominator =
  case boundRational @PositiveUnitInterval (numerator % denominator) of
    Nothing -> error "coefficient outside PositiveUnitInterval representation"
    Just f -> mkActiveSlotCoeff f

bool :: Bool -> String
bool True = "true"
bool False = "false"

rows :: ([Integer] -> [String]) -> IO ()
rows action = do
  eof <- isEOF
  unless eof $ do
    line <- getLine
    putStrLn (unwords (action (map read (words line))))
    rows action

arithmetic :: [Integer] -> [String]
arithmetic [xr, ar, br] =
  [format (exp' x), format (-ln' a), format (1 - exp' (b * c))]
    ++ comparison (taylorExpCmp 3 (1 / (1 - a)) (-(b * c)))
  where
    x = fixed xr
    a = fixed ar
    b = fixed br
    c = ln' (0.9 :: FixedPoint)
arithmetic _ = error "arithmetic expects xRaw aRaw bRaw"

primitive :: String -> [Integer] -> [String]
primitive operation [a, b] = [raw result]
  where
    result = case operation of
      "multiply" -> fixed a * fixed b
      "divide" -> fixed a / fixed b
      "ratio" -> fromRational (a % b)
      _ -> error "unknown primitive"
primitive _ _ = error "primitive expects two integers"

values :: [Integer] -> [String]
values [width, cert, pool, active, fn, fd]
  | width `notElem` [32, 64] = error "VRF width must be 32 or 64"
  | active <= 0 || pool < 0 || pool > active = error "invalid stake fraction"
  | cert < 0 || cert > maximumNat = error "certificate outside bound"
  | otherwise = [bool elected, raw sigma, cOut, raw q, xOut]
  where
    maximumNat = 2 ^ (8 * width)
    f = coefficient fn fd
    elected = checkLeaderNatValue (assertBoundedNatural (fromInteger maximumNat) (fromInteger cert)) (pool % active) f
    sigma = fromRational (pool % active) :: FixedPoint
    c = activeSlotLog f
    q = if cert == maximumNat then fromInteger maximumNat else fromRational (maximumNat % (maximumNat - cert))
    x = -(sigma * c)
    cOut = if fn == fd then "none" else raw c
    xOut = if fn == fd then "none" else raw x
values _ = error "values expects vrfBytes certNat poolStake activeStake fNumerator fDenominator"

hexBytes :: String -> BS.ByteString
hexBytes s
  | odd (length s) || not (all isHexDigit s) = error "invalid hexadecimal nonce"
  | otherwise = BS.pack (go s)
  where
    go [] = []
    go (a:b:rest) = fromIntegral (16 * digitToInt a + digitToInt b) : go rest
    go _ = error "invalid hexadecimal nonce"

hex :: BS.ByteString -> String
hex = concatMap (\b -> let s = showHex b "" in replicate (2 - length s) '0' ++ s) . BS.unpack

schedule :: [String] -> IO ()
schedule [mode, firstText, countText, poolText, activeText, fnText, fdText, dnText, ddText, nonceText] = do
  unless (mode `elem` ["tpraos", "praos"]) (error "unknown schedule mode")
  unless (first >= 0 && count >= 0 && first + count <= 2 ^ (64 :: Int)) (error "invalid slot range")
  unless (active > 0 && pool >= 0 && pool <= active) (error "invalid stake fraction")
  unless (d >= 0 && d <= 1) (error "invalid overlay coefficient")
  putStrLn (hex (VRF.rawSerialiseSignKeyVRF key))
  forM_ [first .. first + count - 1] $ \slot ->
    unless (overlay (slot - first)) $
      if elected slot then putStrLn (show slot) else pure ()
  where
    [first, count, pool, active, fn, fd, dn, dd] = map read [firstText, countText, poolText, activeText, fnText, fdText, dnText, ddText] :: [Integer]
    f = coefficient fn fd
    sigma = pool % active
    d = dn % dd
    -- Slot.isOverlaySlot's rational ceiling formula, with firstSlot as epoch origin.
    overlay offset = mode == "tpraos" && ceiling (fromInteger offset * d) < (ceiling (fromInteger (offset + 1) * d) :: Integer)
    nonce = case hashFromBytes (hexBytes nonceText) of
      Nothing -> error "nonce must have 32 bytes"
      Just h -> Nonce h
    key = VRF.genKeyVRF @(VRF StandardCrypto) (Seed.mkSeedFromBytes (BS.replicate 32 0))
    elected slot
      | mode == "tpraos" = checkLeaderValue (VRF.certifiedOutput (VRF.evalCertified () (mkSeed seedL (SlotNo (fromInteger slot)) nonce) key)) sigma f
      | otherwise = checkLeaderNatValue (Praos.vrfLeaderValue (Proxy @StandardCrypto) (VRF.evalCertified () (Praos.mkInputVRF (SlotNo (fromInteger slot)) nonce) key)) sigma f
schedule _ = error "schedule expects mode firstSlot slotCount poolStake activeStake fNumerator fDenominator dNumerator dDenominator nonceHex"

main :: IO ()
main = do
  args <- getArgs
  case args of
    ["arithmetic"] -> rows arithmetic
    ["values"] -> rows values
    [operation] | operation `elem` ["multiply", "divide", "ratio"] -> rows (primitive operation)
    ["comparison"] -> rows (\xs -> case xs of
      [q, x] -> comparison (taylorExpCmp 3 (fixed q) (fixed x))
      _ -> error "comparison expects qRaw xRaw")
    "schedule" : rest -> schedule rest
    _ -> error "expected arithmetic, values, comparison, multiply, divide, ratio, or schedule mode"
